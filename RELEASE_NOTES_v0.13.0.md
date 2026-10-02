# v0.13.0 — hermes-disk-search

## Главное: миграция ядра на Rust завершена (W1–W5)

- Ядро и резидентные компоненты — **Rust** (`crates/`): `hds-search` (поиск/RAG),
  `hds-mcp` (stdio + HTTP:8787), `hds-ui`, `hds-llama` (`llm-host` — владелец портов
  8010–8012 и GPU, VRAM-диспетчер, ASR), `hds-index` (обход/конвейер/watcher/`db-move`/`diag`),
  `hds-clip` (ONNX), `hds-cli` (`hds`).
- Бинарники релиза: **`hds`**, **`hds_mcp`**, **`llm_host`**.
- Python остаётся **только как `sidecar/`** (извлечение/лемматизация; собирается отдельно).
  Python-ядро и Python-джобы CI удалены.

## Наблюдаемость и устойчивость движка (L1)

- Шлюз к движку (`gate.rs`) с меткой занятости и ограниченными ожиданиями — один зависший
  вызов больше не парализует `status`, арбитр и другие роли.
- Атрибуция VRAM по процессам (PDH, `gpuattr.rs`), heartbeat резидента
  (`data/llm-host.heartbeat.json`), `llm_host stop --force`, пункт `gpu-observability` в
  `hds check` и UI.

## Патч движка v1.15 (наш, временный — до апстрима)

- Класс правок: (P1) ограниченное ожидание слота, (P2) загрузка модели **вне** instance-лока,
  (P3) `set_cluster_error` вне instance-лока (ABBA), (KV) тип KV-кэша + Flash Attention.
- Результат: у чата **KV q8_0 ≈272 МиБ** вместо f16 512 МиБ; compute-буфер embedding в бюджете.
- Патч собран, проверен живьём и внедрён в боевой каталог; поставка — тег `engine-patch-v1`
  (`installers/fetch_engine_runtime.ps1 -PatchEngine`, откат `-RollbackEnginePatch`).
  Апстрим-разбор — `engine-patch/UPSTREAM_REPORT.md`.

## Релиз-инфраструктура (этот выпуск)

- GitHub Actions переписаны под Rust-first: `rust.yml` (CI) и `release.yml` (релиз).
- **Windows x64** — основная платформа (блокирующая); **macOS arm64** — best-effort
  (mac-джобы не блокируют релиз; ассеты добавляются при успешной сборке).
- Ассеты: пакеты `hds-<ver>-{windows-x64,macos-arm64}.zip` (+ sha256), рантаймы движка
  `hds-engine-runtime-{windows-x64-cuda,macos-arm64-metal}.zip`, архивы исходников.
- Правила выпуска — `RELEASING.md` (новый).

## Требования к обновлению

- Поставка ставится с нуля (установщики рассчитаны на раскладку `bin/` + `sidecar/`).
- Боевой `config.yaml` совместим; `--cache-type-k/v` заработают только с патченым движком
  (тег `engine-patch-v1`), без него — просто без эффекта, предупреждений нет.
- После смены модели эмбеддингов — переиндексация (`hds index --full`).
