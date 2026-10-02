# HANDOFF — промт для нового чата (обновлён 02.10.2026: L1 + патч движка)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, состояние, точка входа,
> правила и грабли. Детали — `tools/parity/W4_REPORT.md` §0 и **§15**, `engine-patch/README.md`.

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`w2-llm-host`** (`main` = `9ed8452` не тронут).
Станция: Windows, RTX 3060 12 ГБ, **CUDA Toolkit 13.4**, VS 18 Community (CMake 4.3 в комплекте),
rustc/cargo 1.97.1. Миграция ядра на Rust **завершена (W1–W5)**; Python — только `sidecar`.
**Сверх плана сделано: L1 (наблюдаемость/устойчивость, 3 шага) и L2b (свой патч движка v1.15 —
собран, проверен живьём, внедрён в боевой каталог) — `W4_REPORT.md` §15.**

## Точка входа (5 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -8`   # HEAD — a24109f
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`     # чисто
3. `cargo test --workspace`                                 # 194 passed / 0 failed (+10 ignored)
4. `cargo clippy --workspace --all-targets -- -D warnings`   # 0/0; fmt --check — 0 diff
5. `cargo run -p hds-cli --bin hds -- check --json` — пункт **`gpu-observability`**: heartbeat
   резидента, занят ли движок, «наш процесс / чужие» по VRAM; ожидается `"ok": true`.
6. Прочитать: `tools/parity/W4_REPORT.md` §0 → **§15** → `engine-patch/README.md` → `README.md`
   → `STATUS.md`; далее журналы `W1_REPORT.md`/`W2_REPORT.md`/`W3_REPORT.md`, `tools/parity/README.md`
   (§3 грабли, §4 чек-лист), `MIGRATION_PLAN_RUST.md` (шапка, §10/§11).

## Состояние (02.10.2026)
- Ядро/резидент на Rust (`crates/`); бинарники `hds.exe` / `hds_mcp.exe` / `llm_host.exe`.
- Владелец портов 8010–8012 — `llm-host` (**release**-резидент) и он работает на **патченом**
  движке: чат `LOADED`, KV **q8_0** 272 МиБ (вместо 512 f16), VRAM чата **7717 МиБ**
  (наш процесс 7263 / чужие 454 — видно в `status` и в `hds check` → `gpu-observability`).
- **Патч движка установлен в боевой каталог** (`%APPDATA%\OpenResearchTools\TranscribeOffline\
  Engine`): наши DLL поверх штатных, штатные рядом как `*.orig` (9 файлов). Откат —
  `installers\fetch_engine_runtime.ps1 -RollbackEnginePatch`.
- `index.pause` заказчика **стоит — не снимать без его решения**; наличие проверять **файлом**
  (`Test-Path .\index.pause`), а не по памяти (в передаче 02.10 оно разошлось с фактом).
- `:8787` — **Rust** `hds mcp-http`; legacy-процессы Python (`hds.cli watch` / `hds.cli mcp-http`)
  остановлены 02.10.2026 — `W4_REPORT.md` §14.
- KV-квант работает: в боевом `config.yaml` (он **в .gitignore**) у чата `--cache-type-k/v q8_0`
  (патч движка включает для V Flash Attention); у embedding исправлен compute-буфер
  (`--ubatch-size 8192 → 512`: ~1,4 ГиБ → 90 МиБ).
- Python-ядро и Python-джобы CI **удалены**; в Python — только `sidecar\` (извлечение/лемматизация,
  самодостаточный через `installers\build_sidecar.ps1`; `.mpp` требует Java 11+).
- golden **заморожен** (генератор удалён); паритет — `compare.py` + golden и `crates/*/tests/*`.

## Что можно делать дальше (на выбор заказчика)
1. **Апстрим-отчёт по-английски** (`engine-patch/UPSTREAM_REPORT.md`) — **единственный незакрытый
   пункт L2b**; публиковать (`gh issue create`) только по решению заказчика. Тег `engine-patch-v1`
   уже создан, 10 ассетов залиты (`-PatchEngine` проверен) — повторно публиковать не нужно.
2. **Реальный релиз**: push ветки → `release.yml` (`test-rust` → `build-sidecar` → `build-windows`
   → `fetch-engine-runtime` → `release`). Локальный release-build требует остановки **двух**
   держателей `target\release\*.exe`: `llm_host stop` **и** `hds mcp-http stop` (либо `-SkipBuild`).
3. **Хвосты патча движка — закрыто (02.10.2026):** перепроверено — в хвостах пяти путей
   `set_cluster_error` уже был вне instance-лока (со стока); снят единственный реальный вызов
   под локом (guard `enable_diarization` в `audio_transcriptions_raw`), патч обновлён
   (24 хунка) — `W4_REPORT.md` §16.
4. Любая новая фича/фикс — с тестами и обновлением `tools/parity/W4_REPORT.md` + `STATUS.md`.

## Команды
```powershell
cargo test --workspace                                  # 194/0 (+10 ignored)
cargo clippy --workspace --all-targets -- -D warnings    # 0/0
cargo run -p hds-cli --bin hds -- check --json           # -> gpu-observability (heartbeat/VRAM)
cargo run -p hds-cli --bin hds -- search "запрос" --limit 8
cargo run -p hds-cli --bin hds -- ask "вопрос"
target\release\llm_host.exe status                       # роли + «VRAM по процессам»
target\release\llm_host.exe run                          # резидент (владелец 8010-8012)
target\release\llm_host.exe stop                         # штатно; --force — если движок завис
# патч движка (оверлей): применить / откатить / опубликовать ассеты
powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_engine_runtime.ps1 -PatchEngine
powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_engine_runtime.ps1 -RollbackEnginePatch
powershell -NoProfile -ExecutionPolicy Bypass -File installers\publish_engine_patch.ps1 -SourceDir <bin\Release>
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_rust_release.ps1 -Version 0.1.0 -SkipBuild
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_sidecar.ps1 -OutDir dist\sidecar -SelfTest
# паритет на фикстурах (нужны .venv + фасад :8011 + tools/parity/out/index.db + golden):
cargo test -p hds-search --test search_parity -- --ignored --nocapture
```

## Не переоткрывать (факты)
- Устройство движка — **числовой** `manual_devices_csv` (имя отвергается); движку обязателен
  cwd = каталог движка + вендорские DLL; **кросс-процессной адресации** инстансов нет → всё через
  фасад `:8010–8012` (владелец — `llm-host`).
- Бюджет VRAM — только **NVML**; thinking — **поле запроса** (один чат-инстанс на агента и MCP);
  движок сам применяет шаблон чата (маркеры ставить нельзя); у гибридных моделей KV держат не все
  слои (`full_attention_interval`) → на 12 ГБ `ctx_per_slot ~16384`.
- Whisper: bridge **audio-only** (без `model_path`), модель — GGML `.bin` в
  `metadata_json.whisper_model` (не GGUF), `mode: subtitle` → `.srt`, есть CPU-fallback по VRAM.
- Поиск/RAG — порт `hds/search.py`/`rag.py`; **лемматизация — через Python-воркер** (иначе FTS не
  совпадёт); паритет golden `search_*.json` — 10/10 (на момент W1).
- `.ps1` — **только ASCII** (PS 5.1 без BOM читает как ANSI); в строках `${var}`, а не `$var:`.
- **Движок: наш патч (до апстрима).** В stock v1.15 `--cache-type-k/v` игнорировались (в cluster/
  bridge API нет полей типа KV) → чат всегда f16 (512 МиБ); загрузка модели шла под
  `instance->mutex` (из-за этого инцидент §14: зависший вызов держал единственный мьютекс
  кластера). Наш патч: P1 ограниченное ожидание слота, P2 загрузка вне лока (свой `load_mutex`
  + `ensure_instance_loaded`), P3 порядок блокировок, KV-тип + Flash Attention для квантованного
  V. Наша Rust-сторона **совместима со стоком** (поля добавлены в конец структур API и стоковая
  DLL их просто игнорирует).
- **Наблюдаемость L1:** метка занятости движка (`crates/hds-llama/src/gate.rs`, `try_with(budget)`),
  атрибуция VRAM по процессам через PDH (`gpuattr.rs`; сверено с `Get-Counter` байт-в-байт),
  heartbeat резидента `data\llm-host.heartbeat.json` (живёт даже при молчащем HTTP),
  `llm_host stop --force` (taskkill по pid-файлу), пункт `gpu-observability` в `hds check`/UI.
- **Цифры после правок (замер 02.10.2026):** KV чата 512 → **272 МиБ**; VRAM чата 7905 → **7717 МиБ**;
  embedding compute-буфер ~1,4 ГиБ → **90 МиБ** при `--ubatch-size 512` (оценка в
  `budget::compute_buffer_mib`: `n_ubatch × hidden × слои × 7,5 Б`). `*.orig` в каталоге движка = откат.

## Грабли окружения (стоили времени)
- `git` — **всегда** `--no-pager` (иначе пейджер «съедает» следующие команды).
- PowerShell иногда портит **первый токен** команды (префикс «с» → «команда не найдена») — повторить
  или начать с пробела; кириллица в `Select-String` молча не ищется (ASCII-шаблон или чтение файла).
- **Запущенный резидент блокирует свой exe**: `cargo build/release`/`cargo test` (debug) падают
  `os error 5` при занятом `target\{release,debug}\llm_host.exe`. Держим резидента из **release**;
  для релизной сборки — `llm_host stop` либо `build_rust_release.ps1 -SkipBuild`. **Держателей
  release-exe теперь два**: резидент и `hds.exe` (`mcp-http`) → `llm_host stop` **и** `hds mcp-http stop`.
- `hds mcp-http start` в PowerShell-конвейере (`| Select-Object`) **не возвращает управление**
  (detached-ребёнок держит pipe) — запускать без конвейера.
- `HDS_CONFIG` в сессии PowerShell **персистентна**.
- Живая машина: боевой индекс `D:\...\index.db`, `index.pause` стоит; боевой `config.yaml` тюнили
  (`ctx_per_slot 16384`, `rerank.url 127.0.0.1`, embedding `--ubatch-size 512`); `config.yaml`
  **в .gitignore** — в репо уходит только `config.example.yaml`.
- **Сборка патча движка:** `build_bridge.ps1` staging копирует исходники моста **с сохранением
  mtime** → после правки патча ninja может решить, что объекты свежее источников, и **не
  пересобрать** (`no work to do`, в DLL остаётся старый код). Лечение: удалить `*cluster*.obj`/
  `*bridge*.obj` перед сборкой; проверка — маркерная строка в DLL (`slot wait timeout`).
- **Сборка движка, тулчейн:** VS-генератор + CUDA падает (`The CUDA Toolkit directory '' does not
  exist`) — нужен путь CI: `vcvarsall x64` + `-CmakeGenerator 'Ninja Multi-Config'`; `GGML_NATIVE`
  несовместим с `GGML_BACKEND_DL` → `-DisableGgmlNative`. Сборка воспроизводит апстрим
  (DLL 5,18/0,28 МБ, экспортов 39 = 39).
- **PowerShell-мелочи, стоившие времени:** JSON для `curl.exe` передавать файлом
  (`--data-binary "@file"`) — inline-кавычки калечатся; `*>` пишет файл в **UTF-16** (для чтения —
  `Out-File -Encoding utf8`); `Set-Item -LastWriteTime` не существует (`(Get-Item path).LastWriteTime = ...`).

## Правила
- Python был источником истины для паритета; **Python-ядро удалено** — golden заморожен, сверка —
  `compare.py` + golden и `crates/*/tests/*`.
- Не трогать боевой индекс, `index.pause` и боевой `config.yaml` без решения заказчика.
- **Патч движка — наш и временный:** при апстрим-фиксе переходим на стоковый вариант
  (`engine-patch/README.md`); факт патча держим в `NOTICE.md`, `README.md`, `runtime-manifests/
  engine-patch.json`.
- После каждого шага — тесты + коммит + обновление `tools/parity/W4_REPORT.md` и `STATUS.md`.
- Сначала предложить план по файлам, при согласовании — код.
