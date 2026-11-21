# AGENT_A6_4_HANDOFF_PROMPT.md — A6.4: откат, журнал решений, relink

> **Дата среза:** 21.11.2026. **Ветка:** `agent-privacy-zone`, **HEAD:** `ac64a682`
> («A6.3 — горячее перечитывание config.yaml демоном зеркала»), дерево чистое,
> `origin/agent-privacy-zone` — ahead, **push не делался**.
> **Машина:** Windows 11, PowerShell 5.1, Rust 1.99, Node 24.18,
> Python 3.12 (`sidecar\venv-anonymize`).
>
> **Что это.** Контекст и готовый промт для нового чата: следующий под-этап
> `AGENT_A6_STAGES.md` §2 — **A6.4 «Откат, журнал решений, relink»** (2–3 дн).
> Правило плана: один под-этап = один чат = зелёный тест + коммит.

## 0. Что уже закрыто (A6.1–A6.3)

| Под-этап | Коммит | Что сделано (снаружи) |
|---|---|---|
| A6.1 Диагностика | `018056d1` | `hds_index::diag::check_agent/check_agent_for` (12 пунктов `agent-*`), `hds agent check [--project P] [--json]`; при `agent.enabled: false` — одна строка `[--]`. `hds-index` получил зависимость `hds-agent` (цикла нет) |
| A6.2 Автозапуск | `15b70e84` | `hds agent sync --all` (один процесс, по потоку на проект, свой `sync.lock`/журнал/реестр/воркеры; недоступная папка → `problem` и пропуск; повторный запуск → «уже запущен», код 0); `installers/install_agent_sync_task.ps1` (задача `HermesDiskSearchAgentSync` на вход в систему, `-TaskName/-Start/-Status/-Remove/-StartupFolder`, fallback «Автозагрузка» через `hidden_launch.vbs`) |
| A6.3 Горячий конфиг | `ac64a682` | `LiveConfig` (mtime+размер) между проходами: `agent.sync.*` (интервал со следующего сна) и `index.max_file_mb` на лету (`Mirror::set_sync_settings`, `Mirror::set_max_file_mb`), строка `config` в `sync.log`; `agent.enabled: false` → `stop reason=disabled` (код 0, `index.stop` не трогается); смена `agent.root` → `config root ignored …` + `[!]`, работа на прежнем; пустой/битый YAML → `error config: …` |

**Прогоны на срезе:** `fmt` — 0; `clippy -D warnings` — 0; `cargo test -p hds-cli` — 22 lib + все
наборы (**14** `agent_sync`), 0 failed; `cargo test -p hds-index` — 50 lib + все наборы, 0 failed;
Python — 335 OK (skip 1); npm — 23/23.

**Что осталось в A6:** A6.4 (этот этап), A6.5 `project rm`, A6.6 эксплуатационные находки
(AG-23/25/26/32), A6.7 приёмка + документация, A6.8 K1 (RAG).


## 1. Контракты, на которые опирается A6.4

| Что | Где | Чем пользоваться |
|---|---|---|
| Решения и очередь | `crates/hds-agent/src/results.rs` | `impl Mirror`: `decide(rel, Decision) -> DecisionReport` (551), `compare(rel)` (915), `refresh_label_images` (853), **`rebuild_image(row, Status) -> String`** (715, приватный — новые методы добавлять в тот же `impl`); свободные: `variant_paths`, `read_queue`, `mark_delivered`, `append_message`, `normalize_rel` |
| Константы | `results.rs` 56–73 | `SERVICE_DIR=".hds-agent"`, `BACKUP_SUBDIR="backup"`, `MESSAGES_FILE`, `CONFLICTS_DIR`, `CONFLICT_SUFFIX`, `DELIVERED_FILE` (446) |
| Копии | `results.rs` 10–14 | layout `.hds-agent\backup\<дата-время>\<rel>`; `DecisionReport.backup: Option<PathBuf>` |
| Отчёты | `results.rs` 228–335 | `DecisionReport { orig_rel, anon_rel, status, backup, conflict_copy, message, warnings }`, `RefreshReport`/`RebuiltImage` (Serialize), `CompareReport`, `FileInfo`, `ResultsError` (336–435: `NotFound`, `Kind`, `Io`, `Busy`, `Registry`, `Mirror`) |
| Реестр | `crates/hds-agent/src/registry_db.rs` | `ArtifactRegistry::{open,path,list,by_orig,upsert,insert,set_status_by_orig,set_baseline,list_pending,count}`; `Status::{Synced,PendingAccept,Conflict,Unsupported,TooBig,Orphan,Accepted,Error}`; `Artifact { id, orig_rel, anon_rel, kind, orig_sha256, anon_sha256_issued, anon_sha256_seen, status }` |
| Маркер образа | `crates/hds-agent/src/mirror.rs` | `Anonymizer::read_marker(&Path) -> hds_anonymize::MarkerInfo { marker, project_id, artifact_id }`; формат `<!-- hds-agent/<P>/<id> -->` (mirror.rs 1945, 1982–1996) |
| Зеркало | `crates/hds-agent/src/mirror.rs` | `Mirror::{new,from_config,sync_once}`, `set_sync_settings`, `set_max_file_mb`; `Status::Orphan` — образ исчез или оригинал удалён при правке агента |
| История сессии | `crates/hds-agent/src/run.rs` | `run::clear_history` — **журнал решений им не чистится** (В-A6-6) |
| CLI-образец | `crates/hds-cli/src/cmd/agent_result.rs` | `cmd_result` (29), `describe` (131), `choice` (143), `print_report` (158), `flag_val` (181), `unknown_flag` (196), `usage` (220); коды 0/1/2 |
| UI вкладки | `crates/hds-ui/src/agent.rs` | `pending_json` (977), `compare_json` (1054), `decision_json` (1098), `files_json` (281); `agent::gate()` — 403 в server-профиле; маршруты в `lib.rs` GET 1939–1985 / POST 2074–2140, CSRF-гейт |
| Панель | `crates/hds-ui/src/page.rs` | блок «Результаты» (кнопки «Открыть», «Сравнить», «Принять», «Отклонить») — сюда копии, «Вернуть», «Пересоздать образ», «Показать в Проводнике» |
| Тесты | `crates/hds-cli/tests/agent_result.rs`, `crates/hds-ui/tests/agent_results.rs` (реальный сокет), фикстура `crates/hds-ui/tests/fixtures/mock_anon.py`; unit — `results.rs` `mod tests` (стенд `stand_with_variant`) |

## 2. Границы этапа (не расширять)

* **Не делать** A6.6 (AG-23 атомарность `decide`, AG-25 кэш реестра в UI, AG-26
  относительные пути образа, AG-32 `warn` о неочищенной истории), A6.5 (`project rm`),
  A6.7 (E2E/отчёт/`RELEASE_NOTES`) и A6.8 (K1).
* Решения владельца **не менять**: В-A6-4 (`--stamp` + список копий), В-A6-5 (relink
  только по команде/кнопке), В-A6-6 (JSONL, ротация 1 МБ, `history clear` не трогает),
  В-A6-7 («Показать в Проводнике» отдельной кнопкой с явным согласием; запуск
  приложений не делаем; копирование пути остаётся).
* Контракты `AGENT_A6_STAGES.md` §4: I8 (`decisions.jsonl` — только пути, метки,
  статусы, числа), AG-20 (действия пользователя в `messages.jsonl` не пишутся),
  AG-22 (`secrets.json` только слиянием), AG-27 (`index.stop` не трогать),
  D20/D21 (маршруты под `agent::gate()`, `P` + 4 цифры), I6 (оригиналы меняются
  только по явной команде — здесь это «Вернуть» / «Принять»).

## 3. Методика (что именно сделать)

### 3.1 `hds-agent` — `crates/hds-agent/src/results.rs` (+ новый `decisions.rs`, если удобнее)

1. **Копии.** `pub struct BackupCopy { pub stamp: String, pub rel: String, pub path: PathBuf, pub size: u64, pub mtime: f64 }`
   (`Serialize`) и `pub fn backups(&self, rel: Option<&str>) -> Result<Vec<BackupCopy>, ResultsError>`:
   обход `.hds-agent\backup\<stamp>\<rel>`, свежие штампы первыми, `rel=None` — все копии проекта.
   `rel` пропускать через `normalize_rel`; имя штампа — только компонент каталога (`..`/разделители
   отбрасывать).
2. **`restore(rel, stamp: Option<&str>) -> Result<DecisionReport, ResultsError>`**: выбрать
   копию (`--stamp` точно, иначе самую свежую; неизвестный штамп — `NotFound` с подсказкой),
   оригинал заменить атомарно (`hds_core::config::replace_file`) — на Windows занятый файл →
   `ResultsError::Busy`, состояние **не меняется**; sha нового оригинала должен совпасть с sha
   копии; затем `rebuild_image(row, Status::Synced)`; отчёт + запись в журнал решений.
   В `messages.jsonl` **не писать** (AG-20: результат виден в UI).
3. **`rebuild(rel) -> Result<DecisionReport, ResultsError>`** — «Пересоздать образ»: только
   `Status::Orphan` (у остальных образ либо актуален, либо есть вариант в `pending` — отказ с
   понятным текстом); образ пересоздаётся из **текущего** оригинала тем же `rebuild_image`.
4. **`relink(rel) -> Result<DecisionReport, ResultsError>`** — только по команде: прочитать образ
   в workspace (`anonymizer.read_marker`), проверить `project_id == self.project_id` и что
   `artifact_id` есть в реестре; запись должна быть `Orphan`/`Error`; восстановить
   `Status::Synced` и основание (`upsert` + `anon_sha256_issued/seen`). Внутри `sync_once`
   **не вызывать** (В-A6-5).
5. **Журнал решений** `.hds-agent\decisions.jsonl`:
   `DecisionEntry { ts, action, orig_rel, stamp: Option<String>, status, image: Option<String>, extra: Option<String> }`
   (только пути/статусы/числа — I8), `append_decision(project_root, &entry)` с ротацией при 1 МБ
   (`.1`, по образцу `LOG_ROTATE_BYTES` в `agent_sync.rs`); пишется из `decide` / `restore` /
   `rebuild` / `relink`. `run::clear_history` файл **не трогает** (проверить тестом).

### 3.2 CLI — `crates/hds-cli/src/cmd/agent_result.rs`

* `hds agent result restore --project <P> --path <rel> [--stamp <stamp>] [--json]`
* `hds agent result rebuild --project <P> --path <rel> [--json]`
* `hds agent result relink --project <P> --path <rel> [--json]`
* `restore` без `--stamp` — свежая копия; копии для выбора печатать при ошибке штампа и в `--json`.
* Коды: 0 — успех; 1 — отказ (`Busy`, `NotFound`, «не orphan», нет маркера) с понятным текстом;
  2 — аргументы. `describe()` уже переводит `ResultsError` в текст — расширить, не дублировать.

### 3.3 UI — `crates/hds-ui/src/{agent,page}.rs`, маршруты в `lib.rs`

* `GET /api/agent/backups?project=<P>&path=<rel>` → список копий (штамп, размер, время, sha);
  `path` пустой — все копии проекта.
* `POST /api/agent/restore`, `POST /api/agent/rebuild`, `POST /api/agent/relink` — тело
  `{project, path, stamp?}`; под `agent::gate()` и CSRF-гейтом, как `decision_json`;
  в server-профиле — 403 (D20).
* Панель «Результаты»: список копий с выбором штампа, кнопки «Вернуть» (подтверждение),
  «Пересоздать образ» (только для `orphan`), «Показать в Проводнике» — отдельная кнопка с явным
  согласием (В-A6-7): запускать **только** `explorer /select,<файл>`, никаких приложений;
  «копировать путь» остаётся. Журнал решений в UI не редактируется (только показ, если решим).


## 4. Критерии приёмки (из `AGENT_A6_STAGES.md` §2, строка A6.4)

1. Восстановление выбранного штампа даёт **sha этой копии**, образ пересоздан
   (`anon_sha256_seen == anon_sha256_issued`, статус `Synced`).
2. Журнал решений **ротируется** при 1 МБ и **не чистится** `hds agent history clear`.
3. relink восстанавливает запись **только по команде/кнопке**: в проходе зеркала не срабатывает
   (проверить: `orphan` + маркер на месте → `sync --once` запись не меняет, `relink` меняет).
4. `orphan` пересобирается («Пересоздать образ»), образ появляется, статус `Synced`.
5. Журнал решений — только пути/статусы/числа (I8): скан фикстуры значений — 0 совпадений.
6. «Показать в Проводнике» — отдельное действие с согласием; приложений не запускает.

## 5. Тесты (минимум)

* `hds-agent` unit (`results.rs::tests`, стенд `stand_with_variant`):
  restore (свежий / по штампу / неизвестный штамп / занятый файл → `Busy` и состояние прежнее),
  `backups` (порядок, фильтр по `rel`, игнор `..` в штампе), `rebuild` (orphan → `Synced`, отказ
  для `pending_accept`), `relink` (маркер → `Synced`; чужой/битый маркер → отказ), журнал решений
  (запись, ротация на 1 МБ, `clear_history` рядом — файл цел), I8 (значений нет).
* `crates/hds-cli/tests/agent_result.rs`: коды и `--json` для трёх команд, `--stamp`, отказы.
* `crates/hds-ui/tests/agent_results.rs` (реальный сокет): `GET /api/agent/backups`, `POST
  restore|rebuild|relink` — 200, 400/404/409 и CSRF; server-профиль → 403 (если уже есть
  каркас в `agent_api`/`agent_results`).

## 6. Прогоны

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings        # при `os error 1455` — `-j 2`
$env:HDS_ROOT=$null; $env:HDS_CONFIG=$null
cargo test -p hds-agent
cargo test -p hds-cli
cargo test -p hds-ui
cargo test -p hds-cli --test agent_result
cargo test -p hds-ui --test agent_results
.\sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
npm --prefix sidecar/hds_agent test
```

## 7. Грабли (оплачены — не наступать)

1. `HDS_ROOT`/`HDS_CONFIG` в окружении — снимать перед `cargo test` (§6 п.1 плана).
2. Windows: занятый файл (`os error 32`) — не ошибка логики: `ResultsError::Busy`, состояние не
   меняется, backup не остаётся (уже реализовано в `decide` — повторить в `restore`).
3. `.hds-agent` — зона агента: в сканах I1 исключать; значения в журналах запрещены (I8).
4. `Mirror::rebuild_image` приватный — не делать публичным ради «красоты», новые операции
   добавлять методами `Mirror` в `results.rs`.
5. Полный параллельный линк на этой машине иногда падает с `os error 1455` → `-j 2`/`-j 1`.
6. Консоль PowerShell врёт про кириллицу — проверять файлы `read_files`, а не вывод.
7. Маршруты вкладки — только через `agent::gate()` + CSRF; `P` — `P` + 4 цифры (D21).
8. `index.stop` не трогать (AG-27): ни одна новая операция его не создаёт и не удаляет.

## 8. Промт для нового чата (копировать целиком)

```text
Ты — разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Задача — под-этап A6.4 плана AGENT_A6_STAGES.md: откат, журнал решений, relink.

Сначала прочитай AGENT_A6_STAGES.md (§2 строка A6.4, §3 решения В-A6-4…7, §4 контракты,
§6 грабли), AGENT_A6_4_HANDOFF_PROMPT.md (контекст, контракты, методика) и код:
crates/hds-agent/src/results.rs (decide/compare/rebuild_image/DecisionReport/ResultsError),
crates/hds-agent/src/registry_db.rs (Status, set_baseline/upsert), crates/hds-agent/src/run.rs
(clear_history), crates/hds-cli/src/cmd/agent_result.rs, crates/hds-ui/src/{agent,page}.rs
и маршруты /api/agent/* в lib.rs, тесты agent_result.rs и agent_results.rs.
Срез: ветка agent-privacy-zone, HEAD ac64a682 (A6.1–A6.3 закрыты, дерево чистое).

Сделай:
1. hds_agent::results: backups(rel?) -> Vec<BackupCopy> (.hds-agent\backup\<stamp>\<rel>);
   restore(rel, stamp?) -> DecisionReport (атомарная замена оригинала, Busy не меняет состояние,
   sha копии, rebuild_image(Status::Synced)); rebuild(rel) только для Status::Orphan;
   relink(rel) по маркеру <!-- hds-agent/<P>/<id> --> — только по команде, в проходе не вызывать;
   журнал .hds-agent\decisions.jsonl (JSONL, ротация 1 МБ, только пути/статусы/числа — I8,
   history clear его не трогает).
2. CLI hds agent result restore|rebuild|relink --project P --path REL [--stamp S] [--json];
   коды 0/1/2; restore без --stamp — свежая копия, неизвестный штамп — понятный отказ.
3. UI: GET /api/agent/backups?project=&path=; POST /api/agent/restore|rebuild|relink
   (тело {project, path, stamp?}) под agent::gate() и CSRF (в server-профиле 403);
   панель «Результаты»: список копий с выбором штампа, «Вернуть» (подтверждение),
   «Пересоздать образ» (только orphan), «Показать в Проводнике» — отдельная кнопка с явным
   согласием (только explorer /select,<файл>, без запуска приложений), копирование пути остаётся.
4. Тесты: unit на restore/backups/rebuild/relink/журнал (ротация, clear_history, I8);
   CLI-тесты кодов и --json; сокет-тесты UI-маршрутов (включая 403/CSRF).
5. README — раздел «Результаты»/демон дополнить: «Вернуть» с выбором штампа, журнал решений,
   relink только по команде.

Не делать A6.5/A6.6/A6.7/A6.8 и не менять решения плана. Секреты и значения из базы меток
в журналы, ответы API и сообщения об ошибках не попадают. Прогоны: cargo fmt, clippy
-D warnings (при os error 1455 — -j 2), cargo test -p hds-agent / -p hds-cli / -p hds-ui,
python и npm — зелёные. Коммит: feat(agent): A6.4 — откат, журнал решений и relink
(restore/rebuild/relink). Push не делать.
```
