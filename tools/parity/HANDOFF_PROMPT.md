# HANDOFF — промт для нового чата

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, состояние, точка входа,
> правила и грабли. Актуальные детали — `W4_REPORT.md` §0 и `README.md`.

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`w2-llm-host`** (`main` = `9ed8452` не тронут).
Рабочая станция: Windows, RTX 3060 12 ГБ, rustc/cargo 1.97.1. Миграция ядра на Rust **завершена
(W1–W5)**; Python остался только как `sidecar`.

## Точка входа (5 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -8`
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`
3. `cargo test --workspace`                                 # ожидается 174 passed / 0 failed (+9 ignored)
4. `cargo clippy --workspace --all-targets -- -D warnings`   # 0/0
5. ОБЯЗАТЕЛЬНО прочитать: `tools/parity/W4_REPORT.md` §0 (передача) → `README.md` → `STATUS.md`;
   далее `tools/parity/W4_REPORT.md` §1–§13 и журналы `W1_REPORT.md`/`W2_REPORT.md`/`W3_REPORT.md`,
   `tools/parity/README.md` (§3 грабли, §4 чек-лист), `MIGRATION_PLAN_RUST.md` (шапка, §10/§11).

## Состояние (02.10.2026)
- Ядро/резидент на Rust (`crates/`); бинарники `hds.exe` / `hds_mcp.exe` / `llm_host.exe`.
- Владелец портов 8010–8012 — `llm-host` (**release**-резидент, `target\release\llm_host.exe run`).
- `index.pause` заказчика **стоит — не снимать без его решения**; наличие проверять **файлом**
  (`Test-Path .\index.pause`), а не по памяти: в передаче 02.10 оно разошлось с фактом.
- `:8787` — **Rust** `hds mcp-http` (`target\release\hds.exe mcp-http status`); legacy-процессы
  Python (`hds.cli watch` / `hds.cli mcp-http`) остановлены 02.10.2026 — `W4_REPORT.md` §14.
- Python-ядро и Python-джобы CI **удалены**; в Python — только `sidecar\` (извлечение/лемматизация,
  самодостаточный через `installers\build_sidecar.ps1`; `.mpp` требует Java 11+).
- golden **заморожен** (генератор удалён); паритет — `compare.py` + golden и `crates/*/tests/*`.

## Что можно делать дальше (на выбор заказчика)
1. **Реальный релиз**: push ветки → `release.yml` (`test-rust` → `build-sidecar` → `build-windows`
   → `fetch-engine-runtime` → `release`); локальный release-build требует `llm_host stop` либо CI.
2. **Тег `clip-onnx-v1`** (`installers\publish_clip_models.ps1`) — до релиза, чтобы CLIP скачивался.
3. **Документный долг**: `RELEASE_NOTES_*`, doc-комментарии Rust «порт `hds/…py`».
4. Любая новая фича/фикс — с тестами и обновлением журнала `W4_REPORT.md` + `STATUS.md`.

## Команды
```powershell
cargo test --workspace                                  # 174/0 (+9 ignored)
cargo clippy --workspace --all-targets -- -D warnings    # 0/0
cargo run -p hds-cli --bin hds -- search "запрос" --limit 8
cargo run -p hds-cli --bin hds -- ask "вопрос"
cargo run -p hds-cli --bin hds -- ui --port 8765
target\release\llm_host.exe run                          # резидент (владелец 8010-8012)
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
  (`ctx_per_slot 16384`, `rerank.url 127.0.0.1`).

## Правила
- Python был источником истины для паритета; **Python-ядро удалено** — golden заморожен, сверка —
  `compare.py` + golden и `crates/*/tests/*`.
- Не трогать боевой индекс и `index.pause`. После каждого шага — тесты + коммит + обновление
  `tools/parity/W4_REPORT.md` и `STATUS.md`.
- Сначала предложить план по файлам, при согласовании — код.
