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
