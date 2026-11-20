# AGENT_A4_5_HANDOFF_PROMPT.md — контекст и промт для под-этапа A4.5 (валидатор TR-7.8)

> **Состояние: A4.5 закрыт 20.11.2026** — коммит `e676bb89` (feat) + docs;
> итоги, тесты и отклонения — `AGENT_A4_STAGES.md` §3.5, стенд —
> `tools/parity/out/agent_a42/A45_E2E.md`. Файл оставлен как история под-этапа:
> методика TR-7.8 ниже совпадает с реализацией, кроме двух мест — `[IMAGE_n]`
> проверяются в **варианте** (в образе они стоят по построению), а
> `Snapshot.others` в проверках не участвует (это значения, не метки).
>
> **Кому:** новый чат Cline в режиме Act. **Дата среза:** 20.11.2026.
> **Ветка:** `agent-privacy-zone`, **HEAD кода:** `d0bb9103` (`docs(agent): A4.4
> закрыт`); сам этот файл — docs-коммит поверх (сверить `git log -1 --oneline`),
> `origin/agent-privacy-zone` ahead 62, дерево чистое, push не делался.
> **Что это:** самодостаточный контекст для продолжения **фазы A4** основного
> проекта `PLAN_AGENT_PRIVACY_ZONE.md` («ИИ-агент в анонимной зоне»). Фаза A4 —
> демон двустороннего зеркала `hds agent sync`; она нарезана на под-этапы
> **A4.1…A4.6** (карта и журнал — `AGENT_A4_STAGES.md` §2). A4.1–A4.4 закрыты,
> **следующий — A4.5 (валидатор результата агента, TR-7.8)**. Кнопки вкладки
> «Агент» — это фаза **A5**, автозапуск демона — **A6**: в A4.5 только ядро
> (библиотека + отчёт), без UI.

## 0. Где мы (коротко)

Основной проект — `PLAN_AGENT_PRIVACY_ZONE.md`: пользователь работает с документами
в своей папке, агент — только с анонимными образами в `<agent.root>\P<NNNN>\workspace`,
исходные значения не покидают машину (I1), оригиналы не меняются без решения
пользователя (I6), в журналах нет значений (I8).

Фазы: **A0** спайки/методика, **A1** база меток v2 и режим `mode:"agent"`, **A2**
индекс и MCP-маршрут анонимной зоны, **A3** Node-воркер/крейт `hds-agent`/CLI
агента — **приняты** (`AGENT_A0_A3_CODE_REVIEW.md`). **A4** (текущая) — зеркало
папки проекта и workspace:

| Под-этап | Что делает | Состояние |
|---|---|---|
| A4.1 | Реестр артефактов `data\agent\registry\<P>.db` (TR-7.2) + `hds agent sync --once` | закрыт 19.11.2026 (`00da7c54`) |
| A4.2 | Прямой путь: обход, `wait_stable`, образы docx/xlsx/txt/md/xml, текстовые образы pdf/pptx/mpp, `unsupported`/`too_big`, удаление/переименование | закрыт 20.11.2026 (`81961936`) |
| A4.3 | Обратный путь: `agent_new` → проект, `agent_changed` → `.hds-agent\pending` + снимок основания, маркер, защита от петель `issued`/`seen` | закрыт 20.11.2026 (`d84571ee`) |
| A4.4 | Решения TR-7.6: «Принять» (backup → замена → `accepted` → пересоздать образ), «Отклонить», три исхода конфликта, «файл занят», очередь `.hds-agent\messages.jsonl` | закрыт 20.11.2026 (`43305867`, docs `d0bb9103`) |
| **A4.5** | **Валидатор TR-7.8**: отчёт `.hds-agent\pending\<rel>.report.json` к каждому варианту — метки вне проекта, падеж, непомещённые `[IMAGE_n]`, «текст пропал» > 40 % | **эта задача** |
| A4.6 | Демон: `--interval` (дефолт `agent.sync.interval_sec` = 5 с), остановка по `index.stop`/Ctrl+C, журнал `agent-state\sync.log`, README с таблицей настроек, `AGENT_A4_REPORT.md` | последний |

Что уже работает end-to-end (не переделывать, использовать как контракт):

* `hds agent sync --project P --once` — один проход = прямой путь + обратный;
  повторный прогон по неизменной папке — **0 изменений** (критерий A4.2–A4.4);
* образы сохраняют структуру папок и получают имена с метками
  (`Протоколы\Встреча с [PERSON_3].docx`), `pdf/pptx/mpp` → `*.pdf.md`;
* правка образа агентом → вариант `.hds-agent\pending\<rel>` (деанонимизированный,
  с восстановленными `docProps`/картинками из `.hds-agent\base\<rel>`) и статус
  `pending_accept`; оригинал при этом не меняется (I6);
* решения: `hds agent result accept|reject|conflict --project P --path <rel>
  [--choice agent|new|delegate]` — ядро `hds_agent::results`;
* уведомления агенту — `.hds-agent\messages.jsonl` (`document_rejected`,
  `conflict_delegated`), только метки и пути (I8); доставка в сессию — A5.

**Зачем A4.5.** Агент может «пересобрать документ с нуля», выдумать метку,
оставить служебную `[IMAGE_n]` в тексте или потерять половину содержания — и
пользователь увидит это только после «Принять». Валидатор пишет **отчёт рядом с
вариантом** и помечает «проверить» (UI — A5), но **не блокирует** выпуск и не
мешает решениям A4.4: лучше показать подозрение, чем потерять работу агента.

## 1. Состояние репозитория (проверено 20.11.2026)

| Параметр | Значение |
|---|---|
| Ветка | `agent-privacy-zone`, HEAD `d0bb9103`, ahead 61, дерево чистое, push не делался |
| Коммиты A4 | A4.1 `00da7c54`, A4.2 `81961936`, A4.3 `d84571ee`, A4.4 `43305867` (+ docs после каждого) |
| Docs среза | `AGENT_A4_STAGES.md` (§2, §3.1–3.4, §4.2 п.17–22, §5–§7, §8) и этот файл |
| Проверки A4.4 | `cargo fmt --check`, `clippy --workspace --all-targets -- -D warnings` — чисто; `cargo test --workspace` — **123 набора / 830 passed / 0 failed**; python `unittest discover -s sidecar/tests -t sidecar` — **328 OK** (skip 1); `npm --prefix sidecar/hds_agent test` — **23/23** |
| Стенд | `tools\parity\out\agent_a42` (32 документа): `pending`/`base` **пусты**, `sync` — `34 файла, 33 без изменений, 1 unsupported, 0 изменений`; два принятых документа, одно отклонение, 1 запись в `messages.jsonl`; отчёты `A43_E2E.md`, `A44_E2E.md`, логи `a44-*.log` |
| Машина | Windows 11, PowerShell 5.1, Rust 1.99, Node 24.18, Python 3.12 (`sidecar\venv-anonymize`), `llm_host` резидент |

Известный флейк: `agent_acceptance::acceptance_a3_end_to_end` снимает mtime
`~/.cline`, а IDE туда пишет — отдельный прогон зелёный; не пугаться.

## 2. Обязательное чтение (по порядку)

1. `AGENT_A4_STAGES.md` — **целиком**: §2 (карта под-этапов), §3.1–3.4 (что уже
   сделано и какие приняты отклонения), §4.2 (долги, особенно пп.17–22),
   §5 (контракты), §6 (грабли 1–23), §7 (команды), §8 (контекст A4.5).
2. `AGENT_A4_HANDOFF_PROMPT.md` — §4.4 (маркер, снимок основания), §4.5 (обратный
   путь), §4.7 (**TR-7.7/7.8: петли и валидатор**), §6 (критерии готовности A4).
3. `PLAN_AGENT_PRIVACY_ZONE.md` — §5.7 (TR-7.8 и таблица четырёх ситуаций),
   §5.9 (инварианты I1–I8), §5.4 (что видит `disk-search` в анонимной зоне).
4. Код: `crates/hds-agent/src/mirror.rs` (прямой/обратный путь, `make_variant`,
   `join_segments`, `image_map_path`), `crates/hds-agent/src/results.rs`
   (решения A4.4, `cleanup_decision`), `crates/hds-agent/src/registry_db.rs`,
   `crates/hds-core/src/labels.rs`, `crates/hds-anonymize/src/protocol.rs`
   (`Snapshot`), `crates/hds-extract/src/{worker,protocol}.rs`,
   `crates/hds-cli/src/cmd/agent_result.rs` (образец CLI и разбора флагов),
   `crates/hds-cli/tests/agent_result.rs` (e2e на mock-воркере).

## 3. Что уже есть в коде — опора A4.5

| Что нужно валидатору | Где лежит | Как использовать |
|---|---|---|
| Точка вызова | `Mirror::make_variant` (`mirror.rs`) | после успешной сборки варианта и `set_issued(seen, PendingAccept)` — писать отчёт рядом; ошибка валидатора **не** должна отменять выпуск |
| Вариант | `.hds-agent\pending\<rel>` (`Mirror::pending_path`, `pub(crate)`) | **деанонимизированный** документ: реальные значения, не метки |
| Образ (метки) | `<P>\workspace\<anon_rel>` | именно здесь проверяются метки, падеж, `[IMAGE_n]` |
| Снимок основания | `.hds-agent\base\<rel>` + `<rel>.images.json` (`base_path`, `image_map_path`) | текст основания и карта `[IMAGE_n]` → часть пакета (В-4) |
| Метки проекта | `hds_anonymize::Client::snapshot(project_id)` → `Snapshot { rev, labels: [{label, entity_type, forms}], others, keep }` | источник истины для «метка выдумана»; в трейте `Anonymizer` (`mirror.rs`) метода нет — добавить (например `snapshot_labels`) или звать клиент там, где он доступен |
| Разбор меток | `hds_core::labels`: `label_parts`, `gram_to_suffix`, `is_label`, `find_labels`, `split_labels` | **единственный** разбор (AG-1): второй регулярки не заводить; `[PERSON_3]` и `[PERSON_3|дат]` — одна сущность |
| Текст документов | `hds_extract::Worker::extract` (PDF/DOCX/XLSX/PPTX) + `mirror::join_segments`; txt/md/xml — чтение как UTF-8 | в зеркале уже есть `TextExtractor`/`LazyExtractor`; текст нужен только для «текст пропал» |
| Удаление отчёта | `results.rs::cleanup_decision` | там же, где удаляются вариант, основание и `.images.json` |
| Формат вывода | `results.rs::DecisionReport` / `Message` (serde_json), `cmd/agent_result.rs` | те же соглашения: пути и метки, без значений (I8) |



## 4. Задача A4.5 (TR-7.8) с методикой

### 4.1. Файл отчёта

`<папка проекта>\.hds-agent\pending\<rel>.report.json` — рядом с вариантом, один на
документ; перезаписывается при пересборке варианта, удаляется вместе с ним. Формат:

```json
{
  "rel": "06 Переписка\\Письмо 01.md",
  "anon_rel": "06 Переписка\\Письмо 01 [DOC_2].md",
  "ts": 1795166000.5,
  "rev": 7,
  "verdict": "ok",
  "checks": [
    {"name": "labels_outside_project", "verdict": "ok", "detail": "меток вне проекта нет"},
    {"name": "label_grammar", "verdict": "check", "detail": "[PERSON_4|дат] без базовой формы"},
    {"name": "unplaced_images", "verdict": "ok", "detail": "непомещённых [IMAGE_n] нет"},
    {"name": "text_lost", "verdict": "ok", "detail": "потеряно 3 % слов"}
  ]
}
```

Правила: `verdict` = `ok` | `check`; **никаких значений** — только метки, пути и
числа (I8); неизвестные проверки из будущих версий читаются как `ok`
(совместимость, как `Status::Unknown` в реестре).

### 4.2. Четыре проверки (`crates/hds-agent/src/validator.rs`)

1. **`labels_outside_project`** — метка в **образе**, которой нет в снимке проекта
   (`Snapshot.labels[].label` + `Snapshot.others`). Признак: агент выдумал или
   исказил метку. `Snapshot.rev` пишется в отчёт — видно, на каком снимке проверяли.
2. **`label_grammar`** — `[X_n|падеж]` без базовой формы `[X_n]` в том же документе
   и без такой метки в снимке; падежный суффикс проверяется только через
   `hds_core::labels::{label_parts, gram_to_suffix}` (своих регулярок не заводить).
   Нормальный русский текст даёт падежные формы — это не ошибка: `check` ставится,
   только когда базовой метки нет вовсе.
3. **`unplaced_images`** — у `Kind::Image` (`docx`/`xlsx`): метки `[IMAGE_n]`,
   оставшиеся в тексте образа, сверяются с картой `<rel>.images.json`. Метка есть в
   карте, но не заменена картинкой → `check`. У `Kind::TextImage`
   (`*.pdf.md`/`*.pptx.md`/`*.mpp.md`) картинок нет — проверка `ok` с
   `detail: "не применимо"`.
4. **`text_lost`** — доля слов основания, которых нет в варианте, > 40 %:
   нормализация (регистр, пробелы, пунктуация, служебные строки `<!-- страница n -->`,
   маркер), сравнение **по словам** (не по байтам). Текст основания —
   `.hds-agent\base\<rel>`, варианта — `pending\<rel>`; для docx/xlsx/pdf/pptx —
   `hds_extract` (+`join_segments`), для `txt`/`md`/`xml` — чтение как UTF-8.
   Сравнивать **образ** с основанием нельзя: в образе метки, в основании значения.

### 4.3. Вердикт и поведение при ошибках

* `verdict = "check"`, если хотя бы одна проверка `check`; иначе `ok`.
* Отчёт **не блокирует** выпуск: вариант остаётся `pending_accept`, статус не
  меняется, решения A4.4 работают как раньше.
* Ошибка самого валидатора (не читается файл, упал воркер извлечения) — строка в
  `SyncReport::problems` и отчёт с `verdict: "check"`; вариант всё равно выпускается.

### 4.4. Жизненный цикл

* создаётся в `Mirror::make_variant` после `set_issued(seen, PendingAccept)`;
* перезаписывается, когда агент правит образ повторно (вариант пересобирается);
* удаляется в `results.rs::cleanup_decision` — при «Принять», «Отклонить», «Взять
  новую версию», «Поручить агенту» (иначе отчёт переживёт решение).

### 4.5. Видимость (по желанию, если останется время)

`hds agent result report --project P --path <rel> [--json]` — печать отчёта
(человекочитаемо и/или JSON для A5). Не делать — зафиксировать долгом в
`AGENT_A4_STAGES.md` §4.2: UI всё равно A5.

## 5. Порядок работ

1. `validator.rs` + тесты **без Python** (фейковые образы/тексты): нормализация,
   подсчёт доли слов, разбор меток, границы порога (39 % / 41 %).
2. Вызов из `Mirror::make_variant` (отчёт не мешает выпуску) и очистка в
   `results.rs::cleanup_decision`.
3. Тесты на копиях: подделанный вариант ловится (метка вне проекта, `[IMAGE_n]`,
   потеря текста), нормальный — нет; отчёт не блокирует; удаляется решением;
   повторный проход не перезаписывает отчёт без причины.
4. (если время) `hds agent result report` + CLI e2e на mock-воркере.
5. Полный набор проверок §6, e2e стенда, коммиты.

## 6. Критерии готовности A4.5

1. `cargo fmt --check`, `clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, `npm test`, python `unittest` — зелёные.
2. Тесты: подделанный вариант → `verdict: "check"` с нужной проверкой; нормальный →
   `ok`; отчёт не блокирует выпуск (вариант в `pending`, статус `pending_accept`).
3. Отчёт удаляется любым решением A4.4 (`accept`/`reject`/`conflict`).
4. В `report.json` нет ни одного исходного значения (ассерт на фикстуре с
   «Иванов Иван» — I8).
5. e2e стенда: правка образа → `pending` + `report.json`; «Принять» → отчёта нет;
   повторный `sync` — 0 изменений.
6. Журнал и долги обновлены в `AGENT_A4_STAGES.md` (§2, §3.5, §4.2, §6, §7).

## 7. Грабли (адресные для A4.5)

1. **Две разные стороны:** в `pending` — деанонимизированный вариант (значения), в
   `workspace` — образ (метки). Проверки меток — только по образу и снимку; «доля
   текста» — вариант против **основания**, не против образа.
2. **Метки:** только `hds_core::labels`; `[PERSON_3]` и `[PERSON_3|дат]` — одна
   сущность (сравнивать по `label_parts`, не строкой).
3. **`[IMAGE_n]` у текстовых образов** не проверять: там картинок нет, иначе каждый
   `*.pdf.md` станет «подозрительным».
4. **XLSX:** текст — по ячейкам; пустые листы и служебные строки не считать потерей.
   Порог — по словам, не по байтам; нормализация детерминированная.
5. **Ошибка валидатора не валит `make_variant`** — иначе вариант не выпустится и
   агент потеряет работу (отчёт ≠ блокировка, TR-7.8).
6. **Отчёт не должен попадать в обход/индекс:** `.hds-agent` — служебный каталог
   (`is_service_dir`), `walk_files` туда не ходит; не «оптимизировать» это.
7. **Тесты идут параллельно в одном процессе** — временные каталоги с
   `AtomicU32`-счётчиком (`mirror::tests::tmpdir` уже так делает).
8. **Env стенда** (`HDS_ROOT`/`HDS_CONFIG`/`HDS_*_PYTHON`) снимать перед
   `cargo test` (грабля 12 в `AGENT_A4_STAGES.md` §6).

## 8. Окружение и команды (кратко; полный список — `AGENT_A4_STAGES.md` §7)

```powershell
cd c:\Users\Sasha\hermes-disk-search
git --no-pager status; git --no-pager log -3 --oneline
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p hds-agent
cargo test -p hds-cli --test agent_result
cargo test --workspace -- --test-threads=4 *> tools\parity\out\a45-cargo-test.log
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
npm --prefix sidecar/hds_agent test
```

* Стенд: `HDS_ROOT` = `<repo>\tools\parity\out\agent_a42\root`, `HDS_CONFIG` =
  `<repo>\tools\parity\out\agent_a42\config.yaml`; команды A4.4 — `AGENT_A4_STAGES.md` §7.
* Кириллица в командной строке PowerShell 5.1 портится: пути брать из
  `Get-ChildItem` в переменные; логи читать с `-Encoding Unicode`.
* Коммиты: `git -c user.name='Alex Guerchoig' -c user.email='Guerchoig@gmail.com'
  commit -F <файл-сообщения>`; кириллица — только из файла UTF-8.

## 9. Вопросы владельцу (дефолты предложены — можно двигаться без ответа)

| # | Вопрос | Дефолт | Альтернатива |
|---|---|---|---|
| В-8 | Порог «текст пропал» | **40 %** слов основания отсутствует в варианте | 25 % — строже, больше ложных «проверить» |
| В-9 | Проверять `[IMAGE_n]` в текстовых образах (`*.pdf.md`) | **нет** (картинок там нет, проверка «не применимо») | проверять как текст — шум |
| В-10 | Отчёт в CLI | **да**: `hds agent result report [--json]`, если успеваем | только файл, CLI — A5 |
| В-11 | Что делать при `verdict: check` | **ничего не блокировать** (TR-7.8): пометка для UI | запретить «Принять» — отклонено планом |

## 10. Промт для нового чата (копировать целиком, режим Act)

```text
Ты — ведущий разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Это продолжение основного проекта «ИИ-агент в анонимной зоне»
(PLAN_AGENT_PRIVACY_ZONE.md). Ветка agent-privacy-zone, HEAD d0bb9103 (docs A4.4),
дерево чистое, push не делался.

Фаза A4 (демон двустороннего зеркала hds agent sync) идёт под-этапами:
A4.1 (реестр) — 00da7c54, A4.2 (прямой путь) — 81961936,
A4.3 (обратный путь, TR-7.5/7.7) — d84571ee,
A4.4 (решения TR-7.6) — 43305867. Журнал, решения и долги — AGENT_A4_STAGES.md
(§2, §3.1–3.4, §4.2, §5–§7, §8); контекст и методика этого под-этапа —
AGENT_A4_5_HANDOFF_PROMPT.md (§3–§7), этот промт — её §10.

Задача: под-этап A4.5 — валидатор результата агента (TR-7.8). Отчёт к каждому
выпущенному варианту: <папка проекта>\.hds-agent\pending\<rel>.report.json
({"rel", "anon_rel", "ts", "rev", "verdict": "ok"|"check", "checks": [{name,
verdict, detail}]}). Отчёт НЕ блокирует выпуск варианта и не мешает решениям
A4.4 — только помечает «проверить» (UI — A5).

1. Прочитай AGENT_A4_5_HANDOFF_PROMPT.md целиком, затем AGENT_A4_STAGES.md
   (§2, §3.4, §4.2 п.17–22, §5–§7) и AGENT_A4_HANDOFF_PROMPT.md §4.7.
2. Новый модуль crates/hds-agent/src/validator.rs. Четыре проверки:
   а) labels_outside_project — метки образа, которых нет в снимке проекта
      (hds_anonymize::Client::snapshot → Snapshot{rev, labels[].label, others});
   б) label_grammar — [X_n|падеж] без базовой формы [X_n] (разбор только
      hds_core::labels::{label_parts, gram_to_suffix});
   в) unplaced_images — [IMAGE_n], оставшиеся в тексте образа, сверять с картой
      .hds-agent\base\<rel>.images.json (только Kind::Image; для TextImage —
      «не применимо»);
   г) text_lost — доля слов основания (.hds-agent\base\<rel>), отсутствующих в
      варианте (pending\<rel>), > 40 %; текст — hds_extract (+mirror::join_segments)
      для docx/xlsx/pdf/pptx, UTF-8 для txt/md/xml; сравнение по словам.
3. Точка вызова — mirror.rs::make_variant после set_issued(seen, PendingAccept):
   отчёт пишется рядом с вариантом; ошибка валидатора НЕ отменяет выпуск
   (строка в SyncReport::problems + verdict check).
4. Жизненный цикл: отчёт удаляется в results.rs::cleanup_decision (accept/reject/
   conflict), перезаписывается при пересборке варианта.
5. Никаких значений в отчёте — только метки, пути, числа (I8).
6. Проверки: cargo fmt --check, clippy -D warnings, cargo test --workspace,
   npm test, python unittest; тесты на копиях: подделанный вариант ловится
   (метка вне проекта, [IMAGE_n], потеря текста > 40 %), нормальный — нет;
   отчёт не блокирует выпуск и удаляется решением; e2e стенда agent_a42
   (правка образа → pending + report.json, «Принять» → отчёта нет, sync — 0).
7. Коммиты: feat(agent): A4.5 — ...; docs(agent): A4.5 закрыт — ...; в сообщении —
   что проверено. Журнал и долги обнови в AGENT_A4_STAGES.md (§2 — статус A4.5,
   §3.5 — факты, §4.2 — долги, §6 — грабли, §7 — команды, §8 — контекст A4.6).
```

**Проверка среза:** ветка `agent-privacy-zone`, HEAD `d0bb9103`, ahead 61, дерево
чистое; `cargo test --workspace` — 123 набора / 830 passed / 0 failed; python 328 OK
(skip 1); `npm test` 23/23; стенд `agent_a42` — `pending`/`base` пусты, `sync` —
0 изменений.

