# Правила выпуска релиза (RELEASING.md)

> Заменяет прежний `releasing.md` (Python-эпоха). Актуальная платформа — **Rust-first**:
> ядро и резидентные компоненты — крейты `crates/` (`hds`, `hds_mcp`, `llm_host`),
> Python — только `sidecar/` (извлечение/лемматизация; собирается джобами `build-sidecar-*`).
> Рантайм движка ставится отдельно (`runtime-manifests/engine-manifest.json`); наш патч
> движка — тег `engine-patch-v1`; ONNX-модели CLIP — тег `clip-onnx-v1`.

## Когда выпускать релиз

- после завершения функционального блока (фичи) и/или исправления существенных ошибок;
- Rust-проверки зелёные: `cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace`;
- `hds check` на реальной машине без критических проблем;
- ручная проверка сценария «спроси в чате → ответ со ссылками».

## Платформы

- **Windows x64** — основная цель, **блокирующая**: без её сборки релиз не выходит.
- **macOS arm64** — **best-effort** (на 10.2026 «не проверено», `MIGRATION_PLAN_RUST.md` §10.0):
  mac-джобы не блокируют выпуск — их ассеты добавляются к релизу **только если сборка прошла**;
  при падении релиз всё равно публикуется с Windows-ассетами.

## Версионирование

SemVer: `vMAJOR.MINOR.PATCH`.
- **PATCH** — правки ошибок, не меняющие поведение;
- **MINOR** — новые фичи;
- **MAJOR** — несовместимые изменения (схема БД без миграции, API MCP, требования окружения).

Тег = имя релиза (`v0.13.0`). Версия продукта — `hds/__init__.py` (`__version__`).
Релизы **не удаляются** (история версий сохраняется) — повторный выпуск той же версии
требует ручного удаления релиза/тега (`gh release delete <тег> --yes --cleanup-tag`).

## Состав ассетов

| Ассет | Что внутри |
|---|---|
| `hds-<ver>-windows-x64.zip` (+ `.sha256.txt`) | `bin\` (Rust: `hds`/`hds_mcp`/`llm_host`), `installers\`, `runtime-manifests\`, `sidecar\`, `assets\`, скрипты установки; ставится `setup.cmd` |
| `hds-<ver>-macos-arm64.zip` (+ `.sha256.txt`) | `bin/` (Rust), `install_macos.command`, `installers/`, `runtime-manifests/`, `sidecar/`, `shortcuts/` — **best-effort** |
| `hds-engine-runtime-windows-x64-cuda.zip` | рантайм движка (LLM-хост + ASR) для Windows |
| `hds-engine-runtime-macos-arm64-metal.zip` | рантайм движка для macOS (Metal) — **best-effort** |
| `hermes-disk-search-<ver>-windows.zip` / `-macos.zip` | `git archive HEAD` (исходники) |

Отдельные теги (не часть версии продукта):
- `engine-patch-v1` — наш патч движка v1.15 (overlay; `installers/publish_engine_patch.ps1`,
  ставится `fetch_engine_runtime.ps1 -PatchEngine`);
- `clip-onnx-v1` — ONNX-модели CLIP (~850 МБ; `installers/publish_clip_models.ps1`).

## Порядок выпуска

1. Всё, на что ссылаются установщики, должно быть **закоммичено**: архив собирается
   `git archive HEAD`.
2. Локальные проверки:
   ```powershell
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
3. Подготовить заметки `RELEASE_NOTES_<версия>.md` и бампнуть `__version__` в `hds/__init__.py`.
4. Запустить workflow — вручную или тегом:
   ```powershell
   gh workflow run release.yml --ref <ветка> -f version=v0.13.0
   gh run watch
   # либо:  git tag v0.13.0 && git push origin v0.13.0   (release.yml сработает на push тега)
   ```
   Ветку указывать обязательно, если релиз делается не из `main` (например `w2-llm-host`):
   `release.yml` должен существовать на дефолтной ветке — он там есть.
5. Джобы workflow:
   - `test-rust` (Ubuntu, блокирующий: fmt + clippy + тесты) → `test-rust-macos` (best-effort);
   - `build-sidecar-windows` / `build-sidecar-macos` — портативный Python-sidecar;
   - `build-windows` (блокирующий) / `build-macos` (best-effort) — бинарники + упаковка + smoke;
   - `fetch-engine-runtime-windows` / `fetch-engine-runtime-macos` — рантайм движка по манифесту (sha256);
   - `release` — выполняется, если **Windows-джобы успешны**; mac-ассеты подключаются, если
     соответствующие mac-джобы прошли (`if: always() && …windows success`).
6. Проверить страницу релиза: ассеты на месте, заметки корректны.

## Ручной выпуск (fallback, без Actions)

Если CI недоступен — собрать локально и залить вручную:
```powershell
powershell -File installers\build_rust_release.ps1 -Version 0.13.0 -SidecarDir <sidecar>
# macOS (на mac):  bash installers/build_rust_release_macos.sh 0.13.0 <sidecar>
gh release create v0.13.0 --title "v0.13.0 - hermes-disk-search" --notes-file RELEASE_NOTES_v0.13.0.md <ассеты…>
```
Резидент держит `target\release\llm_host.exe` — перед релизной сборкой локально:
`llm_host stop` **и** `hds mcp-http stop` (или `-SkipBuild`).

## Правила good-practice

- **Не выпускать релиз с красными тестами** — `test-rust` блокирует выпуск.
- Заметки на русском: новые фичи, исправления, требования к обновлению
  (например «запустите `index --full` после смены модели»).
- Workflow **никогда не удаляет** прежние релизы и теги.
- macOS-подпись — ad-hoc на целевой машине (`codesign` в `install_macos.command`);
  подписи не коммитятся.
- Откат: `git revert` + повторный запуск workflow с новым PATCH.
