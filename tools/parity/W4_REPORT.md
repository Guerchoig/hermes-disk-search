# W4_REPORT.md — журнал волны W4 (упаковка/установка/CI)

> Ветка `w2-llm-host` (W4 продолжаем в ней), начато **01.10.2026**. План —
> `MIGRATION_PLAN_RUST.md` §10 (артефакты/установка) и §11 (CI).

## 0. Передача в новый чат (02.10.2026)

**Где мы.** Ветка `w2-llm-host` (`main` = `9ed8452` не тронут), **HEAD `a24109f`**.
`cargo test --workspace` — **194 passed / 0 failed (+10 `#[ignore]`)**; `cargo fmt --check` и
`cargo clippy --workspace --all-targets -- -D warnings` — **0/0**, блокирующие в CI.
**Миграция завершена: W1–W5.** Владелец портов 8010–8012 — `llm-host` (release-резидент), и он
работает на **патченом движке** (KV q8_0: 272 МиБ вместо 512 f16; VRAM чата 7717 МиБ);
`index.pause` заказчика **стоит — не снимать**.
**Сверх плана закрыто 02.10.2026: L1** (наблюдаемость, устойчивость, чистка триггеров) и **L2b**
(свой патч движка v1.15 — собран, проверен живьём, внедрён в боевой каталог; оверлей-поставка и
доки) — подробно в **§15**.

**Что закрыто (журналы).**
* **W0** — `SPIKES.md`. **W1** — `W1_REPORT.md` §1–§7 (поиск/RAG/MCP/UI).
* **W2** — `W2_REPORT.md` §9–§17 (A1–A6: устройство/VRAM/диспетчер/фасад/`llm-host`; B1–B7: ядро/watcher/sidecar/CLI).
* **W3** — `W3_REPORT.md` (ASR движком; бюджет VRAM + CPU-fallback; CLIP на ONNX).
* **W4** — этот файл §1–§13: CI+пакет, установщик под `bin\` + задачи/интеграции, доставка
  рантайма движка/GGUF/whisper/CLIP, `package`/`release`, UI-дополнения, диагностика in-process,
  сплит `hds/` (самодостаточный sidecar), корень проекта от exe, версии `app\<ver>`, **`.mpp`/jpype**,
  dry-run пакета, **W5-финал**.
* **W5** — `fmt`/`clippy` блокирующие; Python-ядро и Python-джобы CI удалены; `README.md` под Rust-first.
* **L1/L2b (сверх плана, 02.10.2026)** — §14 (живой порядок на машине: кто держит VRAM) и **§15**
  (L1 шаги 1–3: шлюз/атрибуция VRAM/heartbeat/`stop --force`/`gpu-observability`; KV-квант; патч
  движка — текст, сборка, живая проверка, внедрение в боевой каталог, оверлей-поставка, доки).

**Карта кода (`crates/`).** `hds-core` (config/db/http/**diag**), `hds-extract` (клиент воркера),
`hds-index` (walk/hash/chunker/pipeline/watch/transcribe/sidecar/diag/heartbeat), `hds-llama`
(engine/cluster/ffi/registry/dispatch/facade/host/bridge_audio/whisper/resident/runtime),
`hds-clip` (ONNX), `hds-search` (fts/snippet/rerank/rag), `hds-mcp`, `hds-ui`, `hds-cli` (`hds`).
Python — только `sidecar\` (+ копия 6 модулей `hds\` при сборке sidecar).

**Команды (из корня репозитория).**
```powershell
cargo test --workspace                                  # 194/0 (+10 ignored)
cargo clippy --workspace --all-targets -- -D warnings    # 0/0
cargo run -p hds-cli --bin hds -- check --json           # -> gpu-observability (heartbeat/VRAM)
cargo run -p hds-cli --bin hds -- search "запрос" --limit 8
cargo run -p hds-cli --bin hds -- ask "вопрос"
target\release\llm_host.exe status                       # роли + «VRAM по процессам»
target\release\llm_host.exe run                          # резидент (владелец 8010-8012)
target\release\llm_host.exe stop                         # --force — если движок завис
powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_engine_runtime.ps1 -PatchEngine
powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_engine_runtime.ps1 -RollbackEnginePatch
powershell -NoProfile -ExecutionPolicy Bypass -File installers\publish_engine_patch.ps1 -SourceDir <bin\Release>
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_rust_release.ps1 -Version 0.1.0 -SkipBuild
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_sidecar.ps1 -OutDir dist\sidecar -SelfTest
```

**Что дальше (остаток).**
* **Апстрим-отчёт (EN)** `engine-patch/UPSTREAM_REPORT.md` — единственный незакрытый пункт L2b;
  публикация (`gh issue create`) — только по решению заказчика. Чтобы `-PatchEngine` работал на
  других машинах, один раз выполнить `installers\publish_engine_patch.ps1` (тег `engine-patch-v1`).
* **Эксплуатация:** прогнать реальный релиз (push ветки → `release.yml`); создать тег
  `clip-onnx-v1` (`installers\publish_clip_models.ps1`) до релиза; реальный release-build требует
  остановки **двух** держателей exe (`llm_host stop` **и** `hds mcp-http stop`) либо CI.
* **Хвосты патча движка — закрыто (02.10.2026, §16):** перепроверено по стоку: в хвостах пяти
  путей запросов `set_cluster_error` уже был **вне** instance-лока (`finish_request_locked(...);
  lock.unlock();` строкой выше — так и в v1.15). Реальный остаток P3-класса (guard `enable_diarization`
  в `audio_transcriptions_raw`) снят приёмом P3 — ABBA против `remove_instance` закрыт полностью.
* **Документный долг:** остаточные упоминания Python в `RELEASE_NOTES_*`; doc-комментарии Rust
  «порт `hds/…py`» (провенанс).
* **macOS** — «не проверено» (§10.0): джобы выведены; `tools/parity/MAC_CHECKLIST.md` — постпроектно.
* Готовый промт для нового чата — `tools/parity/HANDOFF_PROMPT.md`.

**Не переоткрывать (факты).** устройство — числовой `manual_devices_csv`; движку нужен cwd = каталог
движка + вендорские DLL; кросс-процессной адресации нет → фасад; VRAM — только NVML; thinking — поле
запроса; KV у гибридных моделей держат не все слои (`full_attention_interval`); whisper: bridge
audio-only, модель — GGML `.bin` в `metadata_json.whisper_model`; лемматизация — через воркер;
golden **заморожен** (W5). **Движок — наш патч до апстрима:** stock v1.15 игнорирует
`--cache-type-k/v` (полей нет в API) и грузит модель под `instance->mutex` (инцидент §14); наш патч
P1/P2/P3+KV, Rust-сторона совместима со стоком. **L1:** метка занятости движка (`gate.rs`),
атрибуция VRAM по процессам (PDH, `gpuattr.rs`), heartbeat `data\llm-host.heartbeat.json`,
`stop --force`, пункт `gpu-observability` в `hds check`/UI.

**Грабли.** `git` — всегда `--no-pager`; PowerShell иногда портит первый токен команды (начать с
пробела/повторить); `Select-String` с кириллицей молча не находит (ASCII-шаблон/чтение файла);
`.ps1` — ASCII-only (PS 5.1 без BOM читает как ANSI), в строках `${var}`, а не `$var:`; резидент
держит свой exe (релиз/тест — `os error 5`; держать из `release` или `-SkipBuild`), держателей
release-exe **два** (резидент и `hds mcp-http`); `HDS_CONFIG` в сессии персистентна.
**Сборка патча:** staging `build_bridge.ps1` сохраняет mtime → ninja может не пересобрать мост
(удалять `*cluster*.obj`/`*bridge*.obj`; проверять маркер `slot wait timeout` в DLL).
**Сборка движка:** нужен `vcvarsall x64` + `Ninja Multi-Config` + `-DisableGgmlNative` (VS-генератор
падает на CUDA, `GGML_NATIVE` несовместим с `GGML_BACKEND_DL`). **PowerShell:** JSON для `curl.exe`
передавать файлом (`--data-binary "@file"`), `*>` пишет UTF-16 (читать `Out-File -Encoding utf8`),
`config.yaml` боевой **в .gitignore**.

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

## 15. L1 — наблюдаемость движка и порядок «кто держит VRAM» (шаг 1, 02.10.2026)

**Зачем (из инцидента §14).** Зависший вызов движка держал единственный мьютекс
(`ClusterShared`), поэтому «резидент не отвечает» стало единственным диагнозом: `status`,
`/internal/status`, арбитр и роли ждали тот же мьютекс, а 10,4 ГБ VRAM держал наш же процесс —
и этого не было видно ни в `status`, ни в `nvidia-smi` (в WDDM per-process память = `N/A`).

**Что сделано (по файлам).**

| Файл | Что внутри |
|---|---|
| `crates\hds-llama\src\gate.rs` (**новый**) | Шлюз к движку: `Busy` (кто/сколько), `with_tagged` (метка занятости, RAII — снимается даже при панике), `try_with(бюджет)` (вместо бесконечного ожидания), восстановление после poisoned-мьютекса, порог «долгого» вызова + `last_slow()` |
| `crates\hds-llama\src\gpuattr.rs` (**новый**) | Атрибуция VRAM по процессам: PDH-счётчики Windows (`PdhAddEnglishCounterW` + `PdhExpandWildCardPathW` через `libloading("pdh.dll")`, без новых зависимостей), `VramAttribution` (наш процесс / чужие / доля), `process_vram_mib(pid)` |
| `crates\hds-llama\src\resident.rs` | Heartbeat резидента: `data\llm-host.heartbeat.json` (`Heartbeat`: pid, uptime, `busy`, `last_slow`, атрибуция VRAM), `read/write/age/is_live/line`, `unix_now()`, `terminate(pid)` (taskkill /T /F — для `stop --force`) |
| `crates\hds-llama\src\host.rs` | `ClusterShared` → обёртка над `Gate<Cluster>`; метки на горячих вызовах (`instances`, `load:<role>`, `wait_loaded:<role>`, `dispatcher`); `uses()`/`props`/`internal_status` с бюджетом 300 мс; в `/internal/status` — `engine_busy` и `engine_last_slow`; поток heartbeat (`spawn_heartbeat`, такт 5 с) + `busy`-переходы в лог; `SLOW_CALL_MS = 30 с` |
| `crates\hds-llama\src\status.rs` | Строка атрибуции в отчёте: «VRAM по процессам: наш процесс X МиБ (Y %), чужие Z МиБ, всего занято W». Считается по запросу (вход отчёта не менялся) |
| `crates\hds-llama\src\bin\llm_host.rs` | `status` при «не отвечает» читает heartbeat и печатает диагноз («кто держит движок, сколько») + подсказку; **`stop --force`** — вежливый `/internal/stop`, затем `taskkill` по pid-файлу и ожидание освобождения (штатный `stop` не может убрать зависший резидент: уборка зовёт `remove_instance` под тем же мьютексом) |

**Приёмка.**
```
cargo test --workspace   -> 190 passed / 0 failed (+10 ignored)   (было 174/+9)
cargo clippy --workspace --all-targets -- -D warnings -> 0/0
cargo fmt --all -- --check -> 0 diff
# живой счётчик (сверка с PowerShell PDH, байт-в-байт):
#   HDS_ATTR_PID=36704 cargo test -p hds-llama --lib gpuattr:: -- --ignored --nocapture
#   -> pid 36704: Some(10922041344) bytes dedicated VRAM
#   Get-Counter '\GPU Process Memory(*)\Dedicated Usage' -> pid_36704: 10922041344 bytes = 10416 МиБ
```

**Что это даёт в инциденте.** Вместо «резидент не отвечает» + слепого прогноза:
`status` покажет «heartbeat не свежий …; движок: занят (wait_loaded:embedding — 3600 с);
VRAM: наш процесс 10416 МиБ, чужие 349 МиБ», а `llm_host stop --force` поднимет машину
без ручного `taskkill`.

**L1 шаг 2 — устойчивость (02.10.2026, тот же день).** Цель: чтобы приоритет
«чат/поиск > индексация» был исполнимым и чтобы чужая загрузка не блокировала систему.

| Файл | Что внутри |
|---|---|
| `crates\hds-llama\src\host.rs` | `ClusterShared::wait_loaded_stepwise` — ожидание готовности **шагами** (`try_with` по 30 с, между шагами замок свободен) вместо «300 с под мьютексом»; используется в `ensure_loaded` и `/internal/load`. `prepare` применяет решение диспетчера через `try_with(DISPATCH_BUDGET = 500 мс)`: занят движок — **вытеснение откладывается** (`[deferred]` в лог и в `decisions`), а запрос **не** отклоняется по нашей неполной оценке (движок умеет выгружать сам). Арбитр — `try_with(ARBITER_BUDGET)`: занят движок, такт пропускается (в §14 арбитр ждал за зависшей загрузкой и не мог вытеснить ничего). `/internal/devices\|load\|unload` — `try_with` с бюджетом и честной ошибкой «движок занят (…)» (клиент видит `503` вместо бесконечного ожидания). |

**Приёмка шага 2.** `cargo test --workspace` — 190/0 (+10 ignored); clippy `-D warnings`
0/0; fmt 0 diff. Смысл правки: «один залипший вызов движка» больше не может заблокировать
`status`, арбитр, вытеснение и другие роли — в худшем случае роль получит честный отказ.

**Живая проверка шага 1 (на боевой машине).** Зависший резидент (3 ч, 10 416 МиБ) снят
командой **`llm_host stop --force`**: `/internal/stop` ответил, но уборка залипла на мьютексе →
через 10 с автоматический `taskkill` → «резидент завершён; VRAM освобождена», VRAM
**10 674 → 361 МиБ**. Новый резидент пишет `data\llm-host.heartbeat.json`
(`{"pid":…,"busy":null,"vram_ours_mib":7501,"vram_foreign_mib":360}`), `status` показывает
строку «VRAM по процессам: наш процесс 7501 МиБ (95 %), чужие 360 МиБ», `/props` отдаёт
`state: LOADED` и `busy: null`.

**KV-квант чата: причина найдена, закрыто на двух сторонах (02.10.2026).** `--cache-type-k/v`
из конфига не работал не из-за конфига: в cluster/bridge API движка **нет полей под тип KV**
(`llama_server_cluster_instance_params` — 22 поля, `llama_server_bridge_params` — тоже; в
реализации ни одного `cache_type`), поэтому KV всегда грузился как f16 (у нашего чата — 512 МиБ
при n_ctx 16384). При этом вендоренный llama.cpp это умеет: `common_params.cache_type_k/v`
(`common/common.h:290`) → `cparams.type_k/type_v` (`common/common.cpp:1380`) →
`llama_context_params.type_k/type_v` (`include/llama.h:353`).
* **наша сторона (сделано, этот шаг):** `parse_kv_type` + распознавание `--cache-type-k/v` в
  legacy `extra_args` и ключей `llm.<role>.cache_type_k/v`; поля в `InstanceSpec`/
  `InstanceParamsRaw` (всегда пишутся явно — `default_instance_params` движок возвращает по
  значению, у стоковой DLL хвост структуры неинициализирован); учёт `KvBits::Q8_0` в оценке
  бюджета. Предупреждение «флаг не распознан» ушло — боевой `config.yaml` заработает без правок.
* **сторона движка (текст готов, ждёт сборки/проверки — `engine-patch/`):** 4 класса правок:
  (P1) `wait_for_instance_slot_locked` → `cv.wait_until(deadline)` (в движке не было ни одного
  `wait_for`); (P2) загрузка модели **вне** `instance->mutex` — вынесена в
  `create_bridge_detached()` + `load_mutex`, а `ensure_instance_loaded()` сам берёт/отпускает
  request-лок короткими секциями; (P3) `set_cluster_error` в `unload_instance`/
  `set_instance_retention_mode` вынесен из-под instance-лока (ABBA против `remove_instance`);
  (KV) +2 поля в cluster+bridge параметрах, присвоение `bridge->params.cache_type_k/v`,
  включение Flash Attention при квантованном V. Патч: `engine-patch/hds-engine-patch-v1.15.patch`
  (4 файла; итерация 2 — 24 хунка), база — тег `v1.15` (`2683eb6`), воспроизведение и откат —
  `engine-patch/README.md`. **Итерация 2 (§16):** остаток P3 закрыт — в хвостах пяти путей
  `set_cluster_error` уже был вне instance-лока (перепроверено по стоку), снят единственный реальный
  вызов под локом (guard `enable_diarization` в `audio_transcriptions_raw`). После сборки: проверка на
  **копии** рантайма + замер KV (`kv_probe`): ожидаем KV ≈ 256 МиБ вместо 512.

**L2b выполнен и проверен живьём (02.10.2026).** Патч собран нашим тулчейном (CUDA Toolkit
13.4 + VS 18 Community + CMake 4.3 в комплекте, генератор **Ninja Multi-Config** после
`vcvarsall x64`, `-Backend cuda -EnableBackendDl -DisableGgmlNative`). Сборка **воспроизводит
апстрим**: `llama-server-bridge.dll` 5,18 МБ против 5,16 у поставленного, `multi-node-server.dll`
0,28 = 0,28; экспортов 39 = 39 → **ABI не менялся**. Живой прогон на **копии** рантайма
(`C:\Users\Sasha\engine-patched`, боевой каталог не тронут; резидент запущен с `--engine-dir`):

* **KV q8_0 заработал** (решение заказчика №2): лог резидента «роль chat: KV **q8_0** 272 МиБ
  (KV-слоёв 8 из 32)» вместо 512 f16; VRAM с загруженным чатом **7905 → 7704 МиБ** (−201 МиБ,
  остаток разницы — Flash Attention, который патч включает для квантованного V);
* **compute-буфер виден бюджету** (решение №1): `--ubatch-size 8192 → 512` в `config.yaml`
  и примере (`--batch-size` 8192 оставлен для скорости индексации) → буфер embedding
  **~1,4 ГиБ → 90 МиБ** (это ровно то, чего прежняя оценка «модель + KV» не видела);
  введена `budget::compute_buffer_mib(meta, n_ubatch)` = `n_ubatch × hidden × слои × 7,5 Б`
  (калибровка по замерам W2: 2048 → ≈1972 МиБ, 512 → ≈469 МиБ; 3 теста);
* живая приёмка патча: чат `POST /v1/chat/completions` → `"content":"ok"` (2 токена),
  эмбеддинги `POST /v1/embeddings` → вектор 1024; `/props` → `state: LOADED`, `busy: null`;
  heartbeat: `наш процесс 7613 МиБ, чужие 440 МиБ`.
* Грабля сборки (стоила времени): `build_bridge.ps1` staging копирует исходники моста
  **с сохранением mtime**, поэтому ninja может решить, что объекты свежее источников, и не
  пересобрать (в DLL остаётся старый код). Лечение: удалить `*cluster*.obj`/`*bridge*.obj`
  перед сборкой и проверять маркерную строку в DLL (`slot wait timeout`). Записано в
  `engine-patch/README.md`.

**Осталось по L2b/L1.** Апстрим-отчёт по-английски; оверлей-ассет (`engine-patch.json` +
`installers/fetch_engine_runtime.ps1` + откат `-RollbackEnginePatch`) и установка патча в
боевой каталог движка (с бэкапами `*.orig`); `NOTICE.md`/README. По L1 шаг 3: убрать
`load_instance` на каждый запрос LOAD_ON_DEMAND, пункт `gpu-observability` в `hds check`/UI,
пометка недостоверного `memory_free` движка (R29) в строке устройств.

**L1 шаг 3 — остаток (02.10.2026).** Сделано:
* `crates/hds-llama/src/dispatch.rs::apply` — для ролей с `retention_mode = LOAD_ON_DEMAND`
  больше **не отправляем** `load_instance` на каждый запрос: движок сам поднимает модель по
  запросу (в логе: «LOAD_ON_DEMAND — загрузку отдаём движку … load_instance не вызываем»).
  Это убирает лишние заходы в путь загрузки, который и подвис в §14; для `KEEP_LOADED` (чат)
  явная загрузка осталась.
* `crates/hds-index/src/diag.rs::check_gpu_observability` — новый пункт **`gpu-observability`**
  в `hds check` (и в UI: он зовёт `run_checks` in-process): «heartbeat N с назад; движок:
  свободен/занят (кто, сколько); VRAM: наш процесс … чужие …; лог резидента N с назад».
  Читает heartbeat-файл, то есть работает **даже когда HTTP резидента молчит** (тот самый
  случай инцидента), и подсказывает `llm_host stop --force`.
* `crates/hds-llama/src/status.rs::device_line` — `memory_free` движка помечен прямо в строке:
  «(R29: `memory_free` движка недостоверен, решения — по NVML)».
Приёмка: живой `hds check --json` → `{"id":"gpu-observability","status":"ok","msg":"heartbeat
3 с назад; движок: свободен; VRAM: наш процесс 7263 МиБ, чужие 454 МиБ; лог резидента 600 с
назад"}`, общий `"ok": true`; тесты 194/0 (+10), clippy 0/0, fmt 0.

**Оверлей-поставка патча (02.10.2026).** Чтобы патч не ставился руками:
* `runtime-manifests/engine-patch.json` — манифест оверлея: база (`base_tag` v1.15, коммит
  `2683eb6`), тег наших ассетов `engine-patch-v1`, 10 файлов (bridge/audio/llama/mtmd/ggml\*)
  с `size`+`sha256`;
* `installers/fetch_engine_runtime.ps1 -PatchEngine` / `-RollbackEnginePatch` — скачивание по
  манифесту с проверкой sha256, бэкап штатных DLL в `*.orig`; откат = восстановление из
  `*.orig` (на каталоге без бэкапов отвечает «nothing to roll back»);
* `installers/publish_engine_patch.ps1` (мейнтейнер, ASCII-only) — берёт вывод патченой сборки
  (`-SourceDir`, `dist/engine-patch/` или `HDS_ENGINE_PATCH_SRC`), **падает при дрейфе хэшей**,
  создаёт тег `engine-patch-v1` и заливает ассеты (`-Clobber` для замены);
* доки: `README.md` — раздел «Патч движка (временно, до апстрима)» и `gpu-observability` в
  «Диагностике»; `NOTICE.md` — наш патч на MIT-базой движка + установка/откат.

Проверки оверлея: `.ps1` — `nonASCII=0`, `parseErrors=0`; манифест ↔ собранные DLL —
`files=10 mismatches=0`; rollback-ветка смоук-прогнана. Rust-проверки не менялись: 194/0 (+10),
clippy 0/0, fmt 0.

**Итог L1 (шаги 1–3 закрыты).** Наблюдаемость (шлюз с меткой занятости, атрибуция VRAM по
процессам, heartbeat резидента, `stop --force`, пункт `gpu-observability`), устойчивость
(шаговое ожидание загрузки, отложенное вытеснение, bounded-арбитр, `503` вместо ожидания) и
чистка триггеров (embedding `--ubatch-size 512`, compute-буферы в бюджете, никакого
`load_instance` на каждый запрос) — сделаны, закоммичены и проверены живьём. Осталось по L2b
только поставка: апстрим-отчёт (EN), оверлей-ассет с откатом в `installers/`, `NOTICE.md`/README.

**Следующее (L2b).** Свой патч движка (4 правки в `bridge/llama_server_cluster.cpp`) на базе
тега **v1.15** (`2683eb69`) + сборка `-Backend cuda` (CUDA Toolkit 13.4, VS 18 с CMake/Ninja) →
оверлей-ассет `engine-patch.json` + `-RollbackEnginePatch`; апстрим-отчёт по-английски.


**Найдено при входе в новый чат (расхождение с передачей).** В §0/`HANDOFF_PROMPT` числилось
«`index.pause` заказчика стоит», но **файла не было**: боевую `D:\hermes-disk-search-db\index.db`
(4,9 ГБ) продолжал индексировать **legacy-Python** — `watch.lock` = 8028,
`pythonw -m hds.cli watch` (запущен 01.10 15:00), `index.heartbeat.json` свежий, `paused: false`,
последняя запись в БД 13:50:17; порт `:8787` держал `pythonw -m hds.cli mcp-http run`
(6084/20728, с 01.10 08:19). Модули ядра (`hds/cli.py`, `mcp_http.py`, `watcher.py`, `indexer.py`)
удалены в §13 → процессы были **неперезапускаемыми зомби** (код только в памяти). При свежем
таймере heartbeat счётчики `seen`/`processed` стояли 12 минут → watcher **подвис**, вероятно на
перегруженном `:8011` (свободно **109 МиБ** VRAM, чат-роль не влезала).

**Решение заказчика (02.10.2026):** «старая python-сборка — рудимент, делай с ней что хочешь».

**Сделано (боевой индекс и его БД не тронуты).**
1. Создан **пустой** `index.pause` (0 байт; presence-файл: Python `os.path.exists`, Rust
   `is_file()`) — задокументированное «пауза стоит». `.gitignore:11` → дерево остаётся чистым.
2. Остановлены 4 процесса `pythonw` (2 службы × шим+воркер: 8028/48588 `watch`,
   6084/20728 `mcp-http run`).
3. Владельцем `:8787` стал штатный Rust-сервер: `target\release\hds.exe mcp-http start`
   (откат — `hds mcp-http stop`).

**Приёмка (живой прогон).**
```
pythonw                     -> процессов нет
8787 -> pid 61552 (hds.exe) |  8010-8012 -> pid 36704 (llm_host.exe)
hds mcp-http status   -> {"pid":61552,"state":"mcp","version":"0.1.0"}
GET /health (8787)    -> {"app":"disk-search","transport":"streamable-http","version":"0.1.0"}
index.heartbeat.json  -> заморожен (14:02:44; llm_host: «нет свежего heartbeat»)
llm_host status       -> «индексация: пауза ДА (...\index.pause)»
nvidia-smi            -> занято 10765/12288 МиБ (было 12005 — освободилось ~1,3 ГБ)
cargo test --workspace -> 174 passed / 0 failed (+9 ignored); clippy 0/0; fmt 0 diff
```
Боевая `index.db` — без записей с 13:50:17 (и после остановки), в БД/`index.pause`-решении ничего
не менялось.

**Грабли (новые).**
* `hds mcp-http start` в PowerShell-конвейере (`| Select-Object`) **не возвращает управление**:
  detached-ребёнок держит pipe (`mcp_http.rs::spawn_instance`). Запускать без конвейера.
* После старта `mcp-http` держателей `target\release\*.exe` **два** — `llm_host.exe` (резидент) и
  `hds.exe` (`mcp-http`): релизная сборка требует `hds mcp-http stop` **и** `llm_host stop`
  (либо CI, либо `-SkipBuild`).
* «Пауза заказчика» — состояние **файла**, а не памяти чата: проверять `Test-Path .\index.pause`
  (в передаче число/факт разошлись).

## 16. Итерация 2 патча движка: хвост P3 закрыт, пересборка, живая проверка (02.10.2026)

**Задача (пункт из §0/§15).** «Хвосты патча движка»: снять `set_cluster_error` из-под instance-лока
в пяти путях запросов (приём P3).

**Находка (уточнение по коду, а не по памяти).** Формулировка «в хвостах пяти путей запросов
`set_cluster_error` под instance-локом» **неверна**: в `chat_complete`, `vlm_complete`, `embeddings`,
`rerank`, `audio_transcriptions_raw` хвостовые `set_cluster_error` вызываются **вне** instance-лока —
`finish_request_locked(*instance); lock.unlock();` стоят строкой выше, и так было уже в **стоке**
`v1.15` (сверено с `git show v1.15:bridge/llama_server_cluster.cpp`; в патче эти строки не менялись).
Аудит (все взятия `instance->mutex` × все вызовы `set_cluster_error`) дал **единственное** реальное
место P3-класса: guard `enable_diarization` в `audio_transcriptions_raw` (ветка не-нативного audio
backend) — `set_cluster_error` исполнялся под `instance->mutex` (лок берётся в начале функции и в
этой ветке не снимался) → ABBA против `remove_instance` (cluster→instance).

**Что сделано (по файлам).**

| Файл | Что внутри |
|---|---|
| `bridge/llama_server_cluster.cpp` (клон v1.15, `C:\Users\Sasha\engine-1.15`) | guard диаризации: `lock.unlock()` перед `set_cluster_error` (в стиле P3); под instance-локом `set_cluster_error` больше не остаётся нигде |
| `engine-patch/hds-engine-patch-v1.15.patch` | **перегенерирован** из клона (`git diff` по 4 файлам `bridge/`, через `cmd` без BOM): теперь **24 хунка**, ~27 КБ; `--reverse --check` против клона — ок |
| `runtime-manifests/engine-patch.json` | новые `sha256` для двух пересобранных DLL (размеры не менялись) |
| `engine-patch/README.md`, `W4_REPORT.md` (§0/§15), `STATUS.md`, `tools/parity/HANDOFF_PROMPT.md` | формулировка исправлена, остаток отмечен закрытым |

**Пересборка (наш тулчейн).** VS 18 `vcvarsall x64` + `Ninja Multi-Config`, `-Backend cuda
-EnableBackendDl $true -DisableGgmlNative $true`, `-LlamaCppDir C:\Users\Sasha\ENGINEbuilds`,
`-BuildDir …\build-bridge-cuda`; перед сборкой удалены stale `*cluster*.obj`/`*bridge*.obj` (грабля
mtime из `engine-patch/README.md`). Пересобрались `llama_server_cluster.cpp`, `llama_server_bridge.cpp`,
`llama_server_multi_node_server.cpp`. Итог — `bin\Release\llama-server-bridge.dll` (**5 432 320 Б**,
sha256 `79d6ec73…`) и `multi-node-server.dll` (**294 400 Б**, sha256 `7593b176…`). Экспорты:
`llama-server-bridge.dll` 113 = 113, `multi-node-server.dll` **39 = 39** → **ABI не менялся**.

**Живая проверка на КОПИИ рантайма** (боевой каталог не тронут; резидент `llm_host` не останавливали —
пробы на CPU/малом офлоаде). Боевой каталог движка скопирован в `tools\parity\out\engine-patched`
(каталог в `.gitignore`), поверх — свежие 2 DLL:
* `kv_probe --engine-dir <copy> --role chat --ngl 8 --n-ctx 4096,32768` → exit 0, модель `LOADED`,
  `Flash Attention … set to enabled`, KV **1.68 КиБ/токен/слой** против формулы f16 4.00 (≈ −58 %) → KV q8_0.
* `chat_probe --engine-dir <copy> --role chat --ngl 0 --n-ctx 4096 --n-predict 16` → exit 0, `ok=true`,
  корректный ответ (`chatml` содержит '4').
* `cargo test --workspace` — **194 passed / 0 failed (+10 ignored)**; `clippy -D warnings` 0/0; `fmt` 0 diff.

**Внедрено и опубликовано (02.10.2026, по решению заказчика).**
* Боевой каталог: `llm_host stop` → подмена `llama-server-bridge.dll` (`79d6ec73…`) и
  `multi-node-server.dll` (`7593b176…`) из свежей сборки → `llm_host run`. Проверено: резидент
  поднялся, `chat LOADED` (VRAM 7682 МиБ), `POST /v1/chat/completions` отвечает; `index.pause` соблюдён.
  (Штатные `*.orig` от итерации 1 рядом — откат `fetch_engine_runtime.ps1 -RollbackEnginePatch`.)
* Публикация: `installers/publish_engine_patch.ps1 -SourceDir …\bin\Release` → тег **`engine-patch-v1`**
  создан, 10 ассетов залиты. Валидировано end-to-end: `fetch_engine_runtime.ps1 -PatchEngine
  -EngineDir <temp>` скачал 10 файлов, сверил sha256, установил (bridge-hash = `79d6ec73…`) — значит
  `-PatchEngine` теперь работает и на других машинах. По ходу — фикс `publish_engine_patch.ps1`
  (грабля: `gh release view` под `$ErrorActionPreference=Stop` давал `NativeCommandError` на
  «release not found» до чтения `$LASTEXITCODE`; в gh-блоке EAP переведён в `Continue`).

**Остаётся.** Только апстрим-отчёт (EN) — `engine-patch/UPSTREAM_REPORT.md`.

