# W4_REPORT.md — журнал волны W4 (упаковка/установка/CI)

> Ветка `w2-llm-host` (W4 продолжаем в ней), начато **01.10.2026**. План —
> `MIGRATION_PLAN_RUST.md` §10 (артефакты/установка) и §11 (CI).

## 1. CI для Rust + релизная сборка (01.10.2026)

**Что сделано.**

| Файл | Что внутри |
|---|---|
| `.github/workflows/rust.yml` | `test-rust` (ubuntu: `cargo clippy --workspace` информативно + `cargo test --workspace`), `build-windows` (windows: `cargo build --release -p hds-cli -p hds-mcp -p hds-llama` + upload `hds.exe`/`hds_mcp.exe`/`llm_host.exe`) |
| `installers/build_rust_release.ps1` | релизная сборка + стейджинг `dist\hds-<ver>-windows-x64\` (bin с 3 exe, `config.example.yaml`, `README.md`, `sha256.txt`); ASCII-only |
| `.gitignore` | `dist/` |

**Решения.**
* `clippy` — **информативно**, без `-D warnings`: в воркспейсе 61 предупреждение,
  `cargo fmt` не применён. Очистка предупреждений и форматирование — отдельная
  задача **W5 (очистка)**; блокирующие в CI — только тесты и сборка.
* Python-регрессия остаётся в `ci.yml` (пока Python есть), новая матрица — `rust.yml`.
* Windows-артефакт — один вариант (без whisper-сборки): whisper даёт рантайм движка.

**Живой прогон.**
```
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_rust_release.ps1 -Version 0.1.0
# [w4] staged: dist\hds-0.1.0-windows-x64  (bin\hds.exe, bin\hds_mcp.exe, bin\llm_host.exe,
#                                           config.example.yaml, README.md, sha256.txt)
dist\hds-0.1.0-windows-x64\bin\hds.exe --help   # ok
```

**Грабли (уже видны).** Релизная сборка **требует остановленного резидента**:
живой `target\release\llm_host.exe` (владелец портов) держит свой exe → `cargo build`
падает `os error 5`. Текущая процедура: `llm-host stop` → сборка → запуск. Долгосрочно
(§10.6) — версионные каталоги `app\<ver>\` + указатель `app\current` и перезапуск
задачи; это отдельная задача W4.

**Дальше по W4:** `setup.ps1`/`install_windows.ps1` под Rust-бинарники (`bin\hds.exe`,
автозапуск `llm-host` вместо Python-ролей), доставка ONNX-моделей CLIP и рантайма ASR
(`engine-manifest.json` + sha256, джоба `fetch-engine-runtime`), `package` (zip + sha256),
`release` с бинарными ассетами.

## 2. Установщик под Rust-бинарники + автозапуск llm-host (02.10.2026)

**Решения заказчика (02.10.2026):** (1) `.ps1` — ASCII-only **English**; (2) GGUF-модели —
отдельный `fetch_llm_models.ps1`; (3) CLIP — наши release-ассеты + `clip-manifest.json`;
(4) `default_onnx_dir` → `models\clip_onnx`; (5) инсталлеры правим на `mcp-http restart`
(вместо `restart-if-stale`); (6) Python-джобы в `release.yml` пока оставляем; (7) фиксируем
**фактические** имена бинарников (`hds.exe`/`hds_mcp.exe`/`llm_host.exe`, не `hdsw`).

**Что сделано (по файлам).**

| Файл | Что внутри |
|---|---|
| `setup.ps1` | **переписан** под Rust-раскладку `bin\` (ASCII-only, English): MotW; проверка `bin\hds.exe` (иначе подсказка про §10.3); winget-зависимости (`ffmpeg` авто, `Tesseract` по согласию, VC++ Redist если нет `vcruntime140[_1].dll`); проверка sidecar-воркера (`hello`-рукопожатие; python: `HDS_EXTRACT_PYTHON` → `sidecar\python` → `.venv`, фолбэк §10.2 п.4); `config.yaml` из образца + эвристика отсутствующих дисков; рантайм движка; GGUF/whisper; задачи Планировщика; интеграции; ярлык; `bin\hds.exe check`; `-SmokeTest` (мини-индекс на temp-БД). Флаги: `-SkipModels -SkipEngine -NoAutostart -SkipIntegrations -SmokeTest`. |
| `installers\fetch_engine_runtime.ps1` | **новый**: выбор ассета из `runtime-manifests\engine-manifest.json` по платформе+бэкенду (`auto` → NVIDIA `cuda`, иначе `vulkan`/`metal`), скачивание, **sha256**, распаковка (+flatten single root), `Unblock-File`, штамп `.hds-engine.json`. Идемпотентен (существующий рантайм без штампа не перекачивается). |
| `installers\fetch_llm_models.ps1` | **новый**: GGUF chat/embedding/rerank в общий `%LOCALAPPDATA%\llama-runtime\models\<role>` (модельная часть старого `ensure_llama_runtime.ps1`, **без** llama-server); быстрый путь из LM Studio; `current.json`. |
| `installers\fetch_whisper_model.ps1` | **новый**: `whisper-large-v3-turbo-GGML.bin` в `%APPDATA%\OpenResearchTools\models\…` (опц.; sha256 у HF нет — проверка размера). |
| `runtime-manifests\engine-manifest.json` (+ `-sources.json`) | **новые, коммитятся**: пин upstream **v1.15** (`windows-x64` cuda/vulkan, `macos-arm64` metal) с `url`+`sha256` (копия формата `transcribeoffline`/движка). |
| `installers\install_llm_host_task.ps1` | `-Exe` по умолчанию `target\release\llm_host.exe` → **`bin\llm_host.exe`**; сообщение о сборке → `build_rust_release.ps1`. |
| `install_autostart.ps1` | **переписан**: задачи `HermesDiskSearchWatch` (`bin\hds.exe watch`) и `HermesDiskSearchMcp` (`bin\hds.exe mcp-http run`); удаление legacy pythonw-задач/ярлыков; фолбэк на Startup. |
| `run_ui.ps1` | **переписан**: `bin\hds.exe ui --port 8765` + ожидание `/api/status` + открытие браузера; логи в `%LOCALAPPDATA%\hermes-disk-search`. |
| `run_index.ps1` | **переписан**: `bin\hds.exe index`. |
| `install_hermes.ps1` | **переписан** (ASCII/English): MCP-блок `url` (из `config.yaml mcp_http.*`, **без Python**) либо stdio `bin\hds_mcp.exe`; `hds mcp-http restart`; валидации — regex (вместо `yaml.safe_load`); NO_PROXY-блок в `.env` (ASCII-маркеры, idempotent). |
| `install_cline.ps1` | **переписан** (ASCII/English): python для `cline_mcp_merge.py` (`HDS_EXTRACT_PYTHON` → sidecar → `.venv`); URL из `config.yaml`; `hds mcp-http restart`. |
| `installers\install_windows.ps1`, `setup.cmd` | текст/проверки под Rust-артефакт (ASCII). |

**Приёмка (безопасная, боевое не тронуто).** Все новые/переписанные `.ps1` — `BOM=False,
nonASCII=0`; `Parser::ParseFile` — без ошибок; JSON-манифесты валидны; `cargo test --workspace`
— **170/0 (+9 ignored)**.
```
powershell -File installers\fetch_engine_runtime.ps1
# [..] engine runtime: tag v1.15, backend cuda -> target: %APPDATA%\…\TranscribeOffline\Engine
# [ok] engine runtime already present (no manifest stamp; use -Force to re-fetch)   exit 0
powershell -File installers\fetch_engine_runtime.ps1 -Backend nosuchbackend
# backend 'nosuchbackend' is not available for 'windows-x64' (tag v1.15); available: vulkan, cuda   exit 1
powershell -File installers\fetch_llm_models.ps1 -Models bogus
# [--] unknown model role: bogus   exit 0
# mcpUrl из живого config.yaml -> http://127.0.0.1:8787/mcp
```
Полный `setup.ps1` интерактивен (Read-Host) и требует `bin\`-артефакт — end-to-end в этой
сессии не прогонялся; синтаксис проверен, все вызовы fetch-скриптов протестированы отдельно.

**Дальше:** `clip-manifest.json` + `installers\fetch_clip_models.ps1` + `default_onnx_dir`
→ `models\clip_onnx` (шаг 2); `package` (zip+sha256) и `release.yml` с бинарными ассетами и
джобой `fetch-engine-runtime` (шаг 3); UI-дополнения и W5 (шаг 4).

## 3. Доставка ONNX-моделей CLIP (02.10.2026)

**Что сделано (по файлам).**

| Файл | Что внутри |
|---|---|
| `runtime-manifests\clip-manifest.json` | **новый, коммитится**: 3 ассета (`vision/clip_vision.onnx` 335 МБ, `text/clip_text_dense.onnx` 516 МБ, `text/tokenizer.json` 2 МБ) с `url` (release-ассеты `Guerchoig/hermes-disk-search@clip-onnx-v1`) и `sha256`, посчитанными с локального экспорта W3. |
| `installers\fetch_clip_models.ps1` | **новый** (ASCII): скачивает по манифесту в `models\clip_onnx`, проверяет **sha256**, идемпотентен (по хэшу), `curl`-фолбэк. |
| `installers\publish_clip_models.ps1` | **новый** (ASCII, для мейнтейнера): через `gh` создаёт тег `clip-onnx-v1` (если нет), сверяет локальные хэши с манифестом (падает при дрейфе) и загружает 3 ассета (`-Clobber` — замена существующих). |
| `crates\hds-clip\src\lib.rs` | `default_onnx_dir()`: `models\clip_onnx` (поставка) → dev-фолбэк `tools\parity\out\clip_onnx` (экспорт W3) → `models\clip_onnx`. `ClipConfig::from_config`/`resolve_tokenizer` подхватывают автоматически. |
| `setup.ps1` | шаг «CLIP ONNX models (image search, optional)» (по согласию, ~850 МБ). |
| `config.example.yaml` | комментарий `index.clip_*` → `models\clip_onnx` + dev-фолбэк. |

**Приёмка.** `.ps1` — `parse-ok`, `nonASCII=0`; `clip-manifest.json` — валиден;
`cargo test --workspace` — **170 passed / 0 failed (+9 ignored)** (тест деградации
`clip_core::missing_models_degrade` не затронут — он задаёт пути явно).

**Оговорка (честно).** `sha256` в манифесте посчитаны с текущего локального экспорта;
release-тег `clip-onnx-v1` ещё **не создан** — перед публикацией релиза выполнить
`installers\publish_clip_models.ps1`. Скачивание end-to-end в этой сессии не прогонялось
(нет опубликованного тега) — проверены синтаксис, манифест и сопоставление путей.

**Дальше:** `package` (zip + sha256) и `release.yml` с бинарными ассетами и джобой
`fetch-engine-runtime` (шаг 3); UI-дополнения и W5 (шаг 4).

## 4. `package` (zip + sha256) и `release` с бинарными ассетами (02.10.2026)

**Что сделано (по файлам).**

| Файл | Что внутри |
|---|---|
| `installers\build_rust_release.ps1` | **переписан**: стейджинг полной раскладки `dist\hds-<ver>-windows-x64\` (§10.1) — `bin\{hds,hds_mcp,llm_host}.exe`, `installers\`, `runtime-manifests\`, `assets\`, `hermes-skill\`, `sidecar\`, `shortcuts\windows\`, корневые скрипты (`setup.cmd/setup.ps1/install_hermes/cline/autostart/run_ui/run_index`), `config.example.yaml`, `README.md` (и `NOTICE.md`, если есть), `sha256.txt`; затем **zip** (содержимое в корне архива) + `<zip>.sha256.txt`. `-SkipZip` — только стейджинг. ASCII-only. |
| `.github/workflows/release.yml` | **переписан**: `test-py`/`test-macos` (Python, **оставлены** до W5) + `test-rust` (clippy информативно + `cargo test`) + `build-windows` (cargo release → `build_rust_release.ps1` → smoke «распаковали → `hds.exe --help`» → upload) + **`fetch-engine-runtime`** (скачивание рантайма движка по манифесту, проверка sha256, zip-ассет) + `build-macos` (архив исходников) + `release` (`gh release create` с **бинарными** ассетами). Версия нормализуется (`v0.2.0` → `0.2.0` для пути пакета). |
| `releasing.md` | обновлены «Порядок выпуска» и «Ассеты релиза»: бинарный Windows-пакет + рантайм движка + архивы исходников; CLIP-модели — отдельный тег `clip-onnx-v1`. |

**Решения.** Python-джобы в релизе оставлены (решение заказчика №6; уйдут в W5). Рантайм
движка — **отдельный** release-ассет `hds-engine-runtime-windows-x64-cuda.zip` (§10.1 допускает
и «вложение в архив как опцию»); `NOTICE.md` копируется, если появится (атрибуция — отдельная
задача).

**Приёмка.** `release.yml` — **валидный YAML** (jobs: `test-py`, `test-macos`, `test-rust`,
`build-windows`, `fetch-engine-runtime`, `build-macos`, `release`); `build_rust_release.ps1` —
`parse-ok`, `nonASCII=0`; `rust.yml`/`ci.yml` — YAML ок (не тронуты).
**Оговорка (честно).** Функциональный прогон упаковки в этой сессии невозможен: живой resident
`target\release\llm_host.exe` держит exe → `cargo build --release` падает `os error 5` (нужен
`llm_host stop`); end-to-end упаковки и сборка engine-ассета проверяются на CI (`windows-latest`).

**Дальше:** UI-дополнения (полный `hds check` в `/api/diagnostics`, кэш дерева, автозапуск UI)
и W5 (clippy/fmt, `-D warnings`).

## 5. UI-дополнения: полный `hds check`, кэш дерева, автозапуск UI (02.10.2026)

**Что сделано (по файлам).**

| Файл | Что внутри |
|---|---|
| `crates\hds-cli\src\cmd\check.rs`, `main.rs` | `hds check [--json]`: машинный вывод `{"ok":…, "checks":[{id,status,title,msg,fix}]}` (нужен UI). Сами проверки — прежний `run_checks`. |
| `crates\hds-ui\src\lib.rs` | `/api/diagnostics` → **полный** `hds check` + отдельный пункт `llm_host`. Проверки выполняет `<рядом>\hds.exe check --json`: вынести в общий модуль нельзя — `hds-cli` **уже зависит** от `hds-ui` (подкоманда `hds ui`), получился бы цикл пакетов (проверено: `cyclic package dependency`). |
| `crates\hds-ui\src\tree.rs` | Кэш `/api/tree` (`Mutex`, TTL 30 с), `?refresh=1` обходит кэш. |
| `installers\install_ui_task.ps1` | **новый**: задача `HermesDiskSearchUi` → `bin\hds.exe ui --port 8765` (`-Status`/`-Remove`, фолбэк на Startup). ASCII-only. |
| `setup.ps1` | опциональный вопрос «Start the web UI automatically at logon?» → `install_ui_task.ps1`. |

**Живой прогон (боевое не тронуто; резидент `llm_host` pid владеет 8010–8012).**
```
target\debug\hds.exe check --json
# json-ok ok=false checks=9: db:ok roots:ok chat:warn emb:fail ocr:ok ffmpeg:ok
#                              lemmatizer:ok rerank:ok python-only:warn
# UI на :8799 (тот же бинарь рядом):
GET /api/diagnostics -> checks=10 (9 из CLI + llm_host), config=…\config.yaml
GET /api/tree        -> dirs=7797; повторный (кэш) -> dirs=7797
```
`chat:warn`/`emb:fail` при живом резиденте — это поведение **самого** `hds check` (сверка роли
по `/props`), не связано с UI; паритет CLI↔UI — цель шага. `cargo test --workspace` — **170/0
(+9 ignored)**.

**Дальше:** W5 — `cargo fmt` + clippy (сейчас 61 предупреждение), `-D warnings` в CI.

## 6. W5 — очистка: `cargo fmt` + clippy, блокирующие в CI (02.10.2026)

**Что сделано.**
* `cargo fmt --all` — весь воркспейс (104 файла).
* `cargo clippy --workspace --all-targets --fix` (авто) + ручные правки: **61 → 0** предупреждений.
* CI (`rust.yml`, `release.yml`): `cargo fmt --all -- --check` и
  `cargo clippy --workspace --all-targets -- -D warnings` — **блокирующие** (добавлен
  компонент `rustfmt`, комментарии обновлены).

**Правки clippy (суть).**

| Линт | Где | Как |
|---|---|---|
| `too_many_arguments` | `hds-core::db::add_chunk`, `hds-index::pipeline::process_file` | дословные порты Python-сигнатур → `#[allow]` с комментарием |
| `while_let_loop` | `hds-core::http::decode_chunked`, `hds-search::rag::strip_think` | переписано на `while let` |
| `needless_range_loop` | `hds-index::chunker::split_text` | `.iter().enumerate().skip()` |
| `manual_checked_ops` | `hds-llama::gguf::head_dim`, `hds-llama::status` | `checked_div(..).unwrap_or(0)` |
| `large_enum_variant` | `hds-llama::registry::RolePlan` | `#[allow]` (горячий план, боксить не стоит) |
| `missing_transmute_annotations` | `hds-core::db::register_vec0` | `#[allow]` (документированный приём sqlite-vec, спайк 1) |
| `type_complexity` | `hds-index/tests/pipeline_incremental.rs`, `hds-llama/src/bin/chat_probe.rs` | `#[allow]` на statement |
| `ptr_arg` | `hds-llama/tests/status_report.rs` | `&PathBuf` → `&Path` |
| `blocks_in_conditions` | `hds-search/tests/search_parity.rs` | блок вынесен в `let same_set` |
| авто (`--fix`) | разные | needless `as_bytes`, бесполезный cast, `div_ceil`, `OR`-диапазон и пр. |

**Приёмка.**
```
cargo fmt --all -- --check                               # 0 diff
cargo clippy --workspace --all-targets -- -D warnings     # 0 warnings / 0 errors
cargo test --workspace                                    # 170 passed / 0 failed (+9 ignored)
```

**Остаётся по плану (вне W4/W5):** `NOTICE.md` (атрибуция движка/CUDA/FFmpeg/pdfium), джобы
`build-sidecar`/`build-macos` (mac — «не проверено», §10.0), версионные каталоги `app\<ver>`
(§10.6). `hdsw.exe`/`hds serve` не вводим — фактические имена (`hds.exe` + подкоманды,
`llm_host.exe`, `hds_mcp.exe`).

## 7. Диагностика in-process (общий код `hds-index::diag`) + `NOTICE.md` (02.10.2026)

**Зачем.** В §5 проверки в UI делались запуском `<рядом>\hds.exe check --json` (обход цикла
`hds-cli → hds-ui`). Работало, но зависело от наличия `hds.exe` рядом и поднимало второй
процесс. Переносим логику в **`hds-index`** (он же зависимость `hds-ui` — цикла нет): один
источник проверок, вызов в процессе.

**Что сделано (по файлам).**

| Файл | Что внутри |
|---|---|
| `crates\hds-index\src\diag.rs` | **новый**: перенос `hds-cli::cmd::check` и помощников (`probe_role`, `role_addr`, `resolve_model`, `runtime_dir`, `props_context`, `which`, `tesseract_ready`, `norm_path`, `Probe`, `PROBE_TIMEOUT`) + `Check`/`run_checks`. `check_db`/`check_roots` — `pub` (чистые, для тестов). |
| `crates\hds-index\src\lib.rs` | `pub mod diag;`. |
| `crates\hds-cli\src\support.rs` | перемещённые помощники **удалены**; `pub use hds_index::diag::{…}` — прежний путь `hds_cli::support::…` сохранён (тесты/подкоманды без изменений). |
| `crates\hds-cli\src\cmd\check.rs` | тонкая обёртка: печать + `--json` над `hds_index::diag::run_checks`. |
| `crates\hds-ui\src\lib.rs` | `/api/diagnostics` — **in-process** `hds_index::diag::run_checks` (убран `run_cli_check`/`std::process`). |
| `crates\hds-cli\tests\check_core.rs` | импорт из `hds_index::diag`. |
| `NOTICE.md` | **новый**: атрибуция (движок, CUDA EULA, FFmpeg, Tesseract, PyMuPDF/AGPL, sidecar-зависимости, модели); копируется в артефакт `build_rust_release.ps1`. |

**Приёмка.**
```
cargo fmt --all -- --check                               # 0 diff
cargo clippy --workspace --all-targets -- -D warnings     # 0 warnings / 0 errors
cargo test --workspace                                    # 170 passed / 0 failed (+9 ignored)
# UI :8799 -> GET /api/diagnostics: checks=10 (db:ok roots:ok chat:warn emb:fail ocr:ok
#   ffmpeg:ok lemmatizer:ok rerank:ok python-only:warn llm_host:warn)
```

**Остаётся (вне W4/W5).** Self-containment Python-sidecar: воркер тянет
`hds.extractors → extract_av/extract_static` (faster-whisper, jpype/mpxj) — нужен сплит `hds/`
(модули извлечения/лемматизации → `sidecar/`), затем джобы `build-sidecar`/`build-macos`;
версионные каталоги `app\<ver>` (§10.6); `.mpp` (jpype/mpxj) пока нет в `requirements.lock`.

## 8. Сплит `hds/`: самодостаточный Python-sidecar (02.10.2026)

**Проблема.** Воркер `sidecar/hds_extract/worker.py` импортировал `hds.*`, а
`hds.extractors` тянет `extract_av`/`extract_static` — в поставке (без Python-ядра) воркер
не работал.

**Решение (без переноса исходников — Python остаётся источником истины).** Воркер кладёт
каталог `sidecar\` в `sys.path` **перед** корнем проекта; `build_sidecar.ps1` собирает
самодостаточное дерево, куда входит **копия** нужных модулей под `sidecar/hds/`. Граф воркера
закрыт ровно `config/extractors/extract_av/extract_static/lemmatizer/whisper_cpp` (импорты
внутри пакета относительные, тяжёлые библиотеки — лениво внутри функций).

**Что сделано (по файлам).**

| Файл | Что внутри |
|---|---|
| `sidecar\hds_extract\worker.py` | `_HERE`/`_SIDE`; `sys.path`: `sidecar\` (копия в поставке) → корень проекта (dev). |
| `installers\build_sidecar.ps1` | **новый** (ASCII): портативный CPython (`uv python install`), зависимости (`uv pip install --break-system-packages` — uv-managed интерпретатор «externally managed»), копия модулей `hds\`, воркер; `-SelfTest`/`-Force`, идемпотентно. |
| `.gitignore` | `sidecar/python/`, `sidecar/hds/`. |
| `installers\build_rust_release.ps1` | `-WithSidecar` (собрать в стейдж) / `-SidecarDir` (взять готовое дерево). |
| `.github\workflows\release.yml` | джоба **`build-sidecar`** (uv, `-SelfTest`, артефакт) → `build-windows` берёт его (`-SidecarDir sidecar_runtime`). |
| `tests\test_ensure_config.py` | 2 теста `RunUiLauncherTests` актуализированы под Rust-`run_ui.ps1`. |

**Приёмка (живой прогон).**
```
installers\build_sidecar.ps1 -OutDir %TEMP%\hds-sc-build -SelfTest   # exit 0, "sidecar-ok"
# воркер запущен ВСТРОЕННЫМ портативным Python 3.12.14 из собранного дерева:
#   hello   -> capabilities [text, pdf, docx, xlsx, pptx, normalize]
#   extract -> kind=text, сегмент с текстом (elapsed 5 мс)
python -m unittest discover -s tests   # 269 OK (skipped=2)   (было 2 падения про старый run_ui.ps1 — поправлены)
cargo test --workspace                 # 170 passed / 0 failed (+9 ignored)
```
Копия самодостаточна: импорт модулей из временного каталога **без корня проекта** — `OK`,
`hds.__file__` указывает на копию.

**Остаётся.** `.mpp` в sidecar требует `jpype1` + `mpxj` (нет в `requirements.lock`) и JDK —
отдельная задача; джоба `build-macos` (mac — «не проверено», §10.0); версионные каталоги
`app\<ver>` (§10.6).

## 9. Корень проекта в поставке: runtime-резолвинг `project_root()` (02.10.2026)

**Найдено живьём — блокер поставки.** `hds_core::config::project_root()` был **build-time**
(`CARGO_MANIFEST_DIR/../..`): распакованная на другой машине сборка искала `config.yaml`,
`sidecar/`, `models/` по пути **машины сборки**. Проверка копии `bin\hds.exe` в temp:
брался `D:\hermes-disk-search-db\index.db` из репозитория вместо собственного `config.yaml`.
На рабочей машине не проявлялось (путь сборки = путь установки).

**Решение.** `project_root()`: `HDS_ROOT` → **рядом с exe** → build-time (dev-фолбэк).
Резолвинг — чистая `root_from_exe` (есть тесты): `<root>\bin\x.exe` → `<root>`;
`target\{debug,release}\{deps\}x.exe` → корень репозитория; иначе — каталог exe.

**Файлы.**

| Файл | Что внутри |
|---|---|
| `crates\hds-core\src\config.rs` | `project_root()` (env → exe → build-time) + `root_from_exe` + 4 юнит-теста (кросс-платформенные). |
| `crates\hds-core\src\db.rs` | `[db] размерность…` → `eprintln!` (stdout у `--json` остаётся чистым). |

**Приёмка.** `cargo fmt --check` 0; clippy `-D warnings` 0/0; `cargo test --workspace` —
**174 passed / 0 failed (+9 ignored**, +4 новых теста); Python-тесты 269 OK.
```
# dev (target\debug):        check --json -> db: D:\hermes-disk-search-db\index.db  (корень репозитория)
# копия в bin\ (temp):       check --json -> db: %TEMP%\hds-ship-probe4\index.db    (корень = каталог поставки)
```

## 10. Версионные каталоги `app\<ver>` + отдельный процесс обновления (02.10.2026)

**Зачем (§10.6).** Запущенный `hds.exe`/`llm_host.exe` на Windows **нельзя перезаписать** —
поэтому версия живёт в отдельном каталоге, а переключается только указатель `app\current`;
обновление выполняет **отдельный** процесс (процесс не может заменить сам себя). Опёрся на
`project_root()` из §9: exe в `app\<ver>\bin\` → корень = каталог версии.

**Раскладка** (внутри `<root>`):

    app\<ver>\        код версии + config.yaml (bin\, installers\, sidecar\, runtime-manifests\, ...)
    app\current       junction -> app\<ver>
    data\             общие данные  (junction из app\<ver>\data)
    models\           общие модели  (junction из app\<ver>\models)

**Файлы.**

| Файл | Что внутри |
|---|---|
| `installers\install_app_version.ps1` | **новый** (ASCII): раскладка `app\<ver>`, junction-ы `data`/`models`, перенос `config.yaml` (активный → корневой → пример), переключение `app\current` (`-SetCurrent`, `-Force`); подсказка про `db_path` (должен быть абсолютным или `data\index.db`). |
| `installers\update.ps1` | **новый** (ASCII): отдельный процесс — stop задач/резидента → `install_app_version.ps1 -SetCurrent` → старт задач; прошлые версии сохраняются (откат = вернуть `current`). |

**Приёмка (живой прогон во временном root).** Собран stage (`bin\hds.exe`, `sidecar\`,
`config.example.yaml`) → `install_app_version.ps1 -Version 0.2.0 -SetCurrent`:
`current` junction → `app\0.2.0`; junction-ы `data`/`models` (write-through в общий `data\`
подтверждён); `config.yaml` перенесён; `app\current\bin\hds.exe check --json` → db
`…\app\current\index.db` (корень = каталог версии через junction). Затем `0.2.1 -SetCurrent`:
`current` → `app\0.2.1`, `config.yaml` подхвачен из активной версии, обе версии на месте.
Скрипты — `parse-ok`, `nonASCII=0`. Rust — 174/0 (+9); Python — 269 OK.

**Остаётся.** `build-macos` (mac — «не проверено», §10.0); release-тег `clip-onnx-v1`.

## 11. `.mpp` (MS Project) в sidecar: `jpype1` + `mpxj` (02.10.2026)

**Проблема.** Python-версия умеет `.mpp` (`hds/extract_static.py::extract_mpp` через mpxj/Java),
но в `sidecar/hds_extract/requirements.lock` не было `jpype1`/`mpxj` — собранный sidecar не
извлекал `.mpp` (паритет форматов неполный).

**Что сделано (по файлам).**

| Файл | Что внутри |
|---|---|
| `sidecar\hds_extract\requirements.lock` | добавлены `jpype1==1.7.1`, `mpxj==16.7.0` (+ комментарий: Java 11+; воркер сам находит `%LOCALAPPDATA%\jdk-21\*\bin\server\jvm.dll` без `JAVA_HOME`; без Java `extract_mpp` деградирует, не падает). |
| `sidecar\hds_extract\worker.py` | `_capabilities()` репортит `mpp` (если `importlib.util.find_spec` видит `jpype`+`mpxj`) — чтобы `hds check`/UI видели деградацию. |
| `installers\build_sidecar.ps1` | self-test импортирует и `jpype, mpxj` (CI ловит отсутствие зависимостей). |
| `crates\hds-index\src\diag.rs` | одна проба воркера (`worker_caps`) → проверки `lemmatizer` и **`mpp`** (пункт 8 `diag.run_checks`); из заметки `python-only` убран `mpxj`. |

**Приёмка (живой прогон).**
```
# .mpp (tools/parity/fixtures/план_проекта_копия.mpp) через воркер:
#   caps: text,pdf,docx,xlsx,pptx,ocr,normalize,mpp
#   kind=mpp, 1 сегмент: "msproj11; начало=2024-11-01T09:00; окончание=2025-02-28T18:00
#                        Согласование договоров по Централизованным Закупкам (MVP); ..."
# hds check --json: db:ok roots:ok chat:warn emb:fail ocr:ok ffmpeg:ok lemmatizer:ok mpp:ok rerank:ok python-only:warn
cargo test --workspace   # 174 passed / 0 failed (+9 ignored)
python -m unittest discover -s tests   # 269 OK (skipped=2)
cargo clippy -D warnings / fmt --check # 0/0 / 0
```

**Оговорка.** В CI `windows-latest` Java может отсутствовать → `extract_mpp` отработает
пункт-заглушкой (тест `.mpp` не должен требовать JVM). Полный паритет `.mpp` проверяется на
машине с JDK (здесь — JDK 21).

## 12. Сборка релизного пакета (dry-run с `-SkipBuild`) (02.10.2026)

**Зачем.** `build_rust_release.ps1` расширяли в §4 под полную раскладку §10.1, но **живьём он
не прогонялся** (живой резидент держит `target\release\llm_host.exe` → `cargo build --release`
падает `os error 5`). Добавлен `-SkipBuild` (взять готовые `target\release\*.exe`) — пакет
собирается, не трогая резидента.

**Прогон.**
```
installers\build_rust_release.ps1 -Version 0.1.0 -SkipBuild
# [w4] -SkipBuild: reusing existing target\release binaries
# [w4] staged:  dist\hds-0.1.0-windows-x64   (46 файлов)
# [w4] package: dist\hds-0.1.0-windows-x64.zip
# [w4] sha256:  93e387a3fea94397276eb045b203a7d34b98dfd681b6ac5f09817449e7bd7484
```
Состав (проверено): `bin\{hds,hds_mcp,llm_host}.exe`; `installers\` (вкл. `install_app_version.ps1`,
`update.ps1`, `build_sidecar.ps1`, `fetch_*`); `runtime-manifests\` (3 json); `sidecar\`;
`assets\ shortcuts\ hermes-skill\`; корневые скрипты + `setup.cmd`; `config.example.yaml`;
`README.md`; `NOTICE.md`; `sha256.txt`. `bin\hds.exe --help` из распакованного дерева работает.

**Оговорка.** Локально `sidecar\` — dev-копия (без портативного Python): полный пакет собирает
CI-джоба `build-sidecar` (или локально `-WithSidecar`, нужен `uv` ~280 МБ загрузки).

## 13. W5-финал: Python уходит из CI и из продукта (02.10.2026)

**Вывод Python-джоб из CI.**
* Удалён `.github\workflows\ci.yml` (Python-only CI на `main`).
* `release.yml`: удалены `test-py`, `test-macos`, `build-macos` (mac-архив исходников) и
  mac-ассет; `release` теперь `needs: [build-windows, fetch-engine-runtime]`. Итоговые джобы:
  `test-rust` (fmt/clippy/tests), `build-sidecar`, `build-windows`, `fetch-engine-runtime`, `release`.

**Удаление legacy Python-обвязки** (продукт от неё не зависел — проверено grep'ом по Rust/PS1/sidecar):
* `hds\` оставлен **только** под sidecar: `__init__.py, config.py, extractors.py, extract_av.py,
  extract_static.py, lemmatizer.py, whisper_cpp.py`. Удалены ядровые модули: `cli.py,
  clip_index.py, chunker.py, db.py, dbops.py, diag.py, embedder.py, indexer.py, llama_runtime.py,
  llama_server.py, mcp_http.py, mcp_server.py, progress.py, rag.py, rerank.py, search.py,
  ui_server.py, watcher.py`.
* `mcp_start.py`, корневой `gen_fixtures.py`, `installers\ensure_llama_runtime.{ps1,sh}`.
* `tests\` (23 файла Python-тестов удалённого ядра).
* `tools\parity\`: удалены зависевшие от ядра генераторы (`golden.py`, `golden_queries.py`,
  `hash_vectors.py`, `walk_parity.py`, `w3_clip_smoke.py`, `spike2_hash.py`).
  **Следствие: golden-файлы заморожены** (регенерация невозможна); `compare.py` и golden остаются эталоном.

**Правки пользовательских строк в Rust** (ссылались на удалённые команды): `hds-index::diag`
(`python-only` → `gpu-manual`, без `python -m hds.cli check`), `hds-index::embed` (хинты →
`bin\llm_host.exe`), `hds-search::rag` (ответ → `hds index`).

**Приёмка.** `cargo fmt --check` 0; clippy `-D warnings` 0/0; `cargo test --workspace` **174 passed
/ 0 failed (+9 ignored)**; урезанный `hds\` обслуживает воркер (temp-копия: `IMPORT-OK`, `hello` →
`text,pdf,docx,xlsx,pptx,normalize,mpp`). Python-тесты удалены вместе с ядром — Python больше не
участвует в CI.

**Долг (осознанно не в этом шаге).** README/`MIGRATION_PLAN_RUST`/`config.example.yaml` и часть
`tools/parity/*` ещё упоминают Python-команды; doc-комментарии Rust вида «порт `hds/…py`» —
историческая провенанс-заметка. mac-артефакт (§10.0, «не проверено») выведен из релиза.
