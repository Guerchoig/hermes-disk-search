# Правила выпуска релиза (RELEASING.md)

> Канонический документ: **как выпускается релиз** `hermes-disk-search` — платформы,
> состав ассетов, версионирование, порядок шагов и грабли. Заменяет прежний
> `releasing.md` (Python-эпоха).
>
> Платформа продукта: ядро и резиденты — Rust (`crates/` → `bin\{hds,hds_mcp,llm_host}`).
> Python остался **только** в извлечении/лемматизации: источники `hds/*.py`, воркер
> `sidecar/hds_extract/worker.py`; в архив кладётся **собранный** self-contained sidecar
> (портативный CPython + копия модулей) — джобы `build-sidecar-*`. Рантайм движка —
> отдельный скачиваемый ассет (`runtime-manifests/engine-manifest.json`); наш патч
> движка — тег `engine-patch-v2` (v1 остался в истории), ONNX-модели CLIP — тег
> `clip-onnx-v1`.

## 1. Когда выпускать релиз

- закрыт функциональный блок (этап `M*`) и/или исправлены существенные ошибки;
- Rust-проверки зелёные **локально и в CI** (`rust.yml`):
  `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`;
- `hds check` на реальной машине без критических замечаний;
- ручная проверка сценария «спроси в чате → ответ со ссылками»;
- в рабочем дереве нет «выпавших» файлов. Архив релиза собирается из `HEAD`, а стейджинг
  копирует дерево, поэтому **случайно удалённый файл ломает выпуск**: пропавшие
  `hds/*.py` или `sidecar/*` роняют сборку sidecar и тест mock-воркера. Перед релизом
  смотреть `git status` и diff по релизным путям.

## 2. Платформы и состав ассетов

- **Windows 10/11 x64** — основная цель, **блокирующая**: и приложение, и **оба**
  офлайн-рантайма движка (CUDA и Vulkan). Без её сборки релиз не выходит.
- **macOS 14+ (Apple Silicon)** — **best-effort** (на 10.2026 на живой машине не
  проверено, `MIGRATION_PLAN_RUST.md` §10.0): mac-джобы **не** блокируют выпуск, их
  ассеты добавляются к релизу, только если сборка прошла; при падении релиз всё равно
  публикуется с Windows-ассетами.
- GPU не обязательна: без NVIDIA движок уходит на Vulkan/CPU. Поэтому под Windows
  публикуются **два** рантайма движка (NVIDIA → CUDA, AMD/Intel → Vulkan), под macOS —
  Metal.

| Ассет | Блокирующий? | Что внутри |
|---|---|---|
| `hds-<ver>-windows-x64.zip` (+ `.sha256.txt`) | **да** | приложение: `bin\` (`hds.exe`, `hds_mcp.exe`, `llm_host.exe`), `installers\`, `runtime-manifests\`, `sidecar\` (портативный CPython + воркер + копия `hds\`), `assets\`, `cline-rules\`, `hermes-skill\`, `shortcuts\windows\`, скрипты установки (`setup.cmd/.ps1`, `install_*.ps1`, `run_*.ps1`), `config.example.yaml`, `README.md`, `NOTICE.md`, `sha256.txt` |
| `hds-engine-runtime-windows-x64-cuda.zip` | **да** | рантайм движка (LLM + ASR) под NVIDIA/CUDA — для **офлайн-установки**; обычный установщик качает его сам по манифесту |
| `hds-engine-runtime-windows-x64-vulkan.zip` | **да** | тот же рантайм под Vulkan (AMD/Intel, машины без NVIDIA) — тоже для офлайн-установки |
| `hds-<ver>-macos-arm64.zip` (+ `.sha256.txt`) | нет (best-effort) | то же приложение для Apple Silicon; лаунчеры стейджатся с `+x` |
| `hds-engine-runtime-macos-arm64-metal.zip` | нет (best-effort) | рантайм движка под Metal |
| `hds-client-<ver>-windows-x64.zip` (+ `.sha256.txt`) | **да** | тонкий клиент (M9): `bin\hds.exe`, мастер `setup-client.*`, правило/скилл, `version.txt`, `sha256.txt`; тот же файл вложен в серверную поставку (`client/`) |
| `hds-client-<ver>-macos-arm64.zip` (+ `.sha256.txt`) | нет (best-effort) | то же для Apple Silicon (мастер `install_client.command`) |
| `hermes-disk-search-<ver>-windows.zip` / `-macos.zip` | нет | `git archive HEAD` — исходники (для сборки из кода) |

Модели (LLM/whisper/диаризация/CLIP ONNX, суммарно ≈11 ГБ) и исходники sidecar в архив
**не входят**: sidecar собирается в CI и кладётся внутрь, модели установщик качает сам.

Отдельные теги (не часть версии продукта):

- `engine-patch-v2` — наш патч движка v1.15 (overlay; `installers/publish_engine_patch.ps1`,
  ставится `fetch_engine_runtime.ps1 -PatchEngine`);
- `clip-onnx-v1` — ONNX-модели CLIP (~850 МБ; `installers/publish_clip_models.ps1`).

**Тонкий клиент (M9) публикуется двумя каналами.** (1) Ассетом релиза —
`hds-client-<ver>-windows-x64.zip` / `-macos-arm64.zip` (+ `.sha256.txt`). (2) Внутрь
серверной поставки, в `<корень>/client/`, откуда сервер отдаёт его по
`<mcp-path>/client-package.zip` (Bearer-гейт как у `/mcp`): клиент качает пакет **своим
токеном**, не ходя на GitHub. Пакет содержит только `bin\hds(.exe)` + правило/скилл +
мастер + `version.txt`; версия объявляется в `/health` и сверяется мастером (W7).

## 3. Версионирование

SemVer: тег `vMAJOR.MINOR.PATCH` — он же имя релиза.

- **PATCH** — исправления ошибок, не меняющие поведение;
- **MINOR** — новые функции;
- **MAJOR** — несовместимые изменения (схема БД без миграции, API MCP, требования
  окружения).

Единственный источник версии продукта — `__version__` в `hds/__init__.py`. Джоба
`check-version` в `release.yml` сверяет её с тегом (или с входом `version`) и **падает
при расхождении** — поэтому «забыли бампнуть версию» выясняется до сборки, а не после
публикации.

Релизы и теги **не удаляются** (история версий сохраняется). Повторный выпуск той же
версии — исключение, только вручную:
`gh release delete <тег> --yes --cleanup-tag` (после этого можно выпустить заново).

## 4. Порядок выпуска

1. **Рабочее дерево.** `git status` чист по релизным путям; ничего нужного не удалено
   (см. §1). Всё, на что ссылаются установщики, должно быть **закоммичено**: архив
   исходников собирается `git archive HEAD`.
2. **Локальные проверки** (то же, что делает блокирующая джоба `test-rust`):
   ```powershell
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
   Мигающие тесты не «глушить» удлинением ожиданий, а чинить детерминированность
   (пример: `waited_ms` быстрого пути очереди — F11, 13.10.2026).
3. **Заметки и версия.** Написать `RELEASE_NOTES_<версия>.md` (с таблицей «что качать» —
   формат в §7) и бампнуть `__version__` в `hds/__init__.py`.
4. **Коммит и пуш ветки.** Убедиться, что `rust.yml` на этой ветке зелёный (для
   `server-multiuser` он теперь запускается).
5. **Тег и выпуск** — тег-пуш (канонический путь) или `workflow_dispatch`:
   ```powershell
   git tag v0.18.0
   git push origin v0.18.0            # release.yml сработает на push тега
   gh run watch                       # следить за прогоном

   # либо вручную с указанием ветки:
   gh workflow run release.yml --ref server-multiuser -f version=v0.18.0
   ```
   Ветку указывать обязательно, если релиз делается не из `main`.
6. **Проверка релиза:** на странице релиза все ассеты на месте (Windows CUDA + Windows
   Vulkan + macOS), `.sha256.txt` рядом, заметки корректны. Если чего-то не хватает —
   прогон не «доделывать руками»: перезапустить workflow (см. §7 про идемпотентность).

## 5. Джобы `release.yml`

| Джоба | Раннер | Блокирует? | Что делает |
|---|---|---|---|
| `check-version` | ubuntu | да | версия тега/входа = `__version__` в `hds/__init__.py` |
| `test-rust` | ubuntu | да | `fmt --check`, `clippy -D warnings`, тесты воркспейса |
| `build-sidecar-windows` / `build-sidecar-macos` | win/mac | win — да | самодостаточный sidecar (портативный CPython + воркер + модули, `uv`) |
| `build-windows` | windows | да | релизные бинарники, стейджинг `dist\hds-<ver>-windows-x64`, вложенный клиентский пакет (`client/`) + `version.txt`, `zip` + `sha256`, smoke (`hds.exe --help`, наличие `client/`) |
| `build-macos` | macos | нет (best-effort) | то же для `macos-arm64` |
| `fetch-engine-runtime-windows-cuda` | windows | да | рантайм движка по манифесту (sha256) под CUDA → `hds-engine-runtime-windows-x64-cuda.zip` |
| `fetch-engine-runtime-windows-vulkan` | windows | да | то же под Vulkan → `hds-engine-runtime-windows-x64-vulkan.zip` |
| `fetch-engine-runtime-macos` | macos | нет (best-effort) | то же под Metal |
| `release` | windows | — | публикует релиз, если **Windows-джобы** успешны (`build-windows` + оба рантайма); mac-ассеты подключаются, если их джобы прошли |

`release` идемпотентен: если релиз уже существует, ассеты перезаливаются
(`gh release upload --clobber`), новый тег не создаётся. При `workflow_dispatch` тег
прибивается к собранному коммиту (`--target $GITHUB_SHA`), а не к вершине default-ветки.

## 6. Ручной выпуск (fallback, без Actions)

Если CI недоступен — собрать локально и залить вручную:

```powershell
# Windows: приложение (sidecar — собранный: -WithSidecar, либо готовый каталог -SidecarDir)
powershell -File installers\build_rust_release.ps1 -Version 0.18.0 -WithSidecar
# macOS (на mac):
bash installers/build_rust_release_macos.sh 0.18.0 <sidecar-dir>

# офлайн-рантаймы движка (оба варианта)
powershell -File installers\fetch_engine_runtime.ps1 -EngineDir dist\engine-cuda   -Backend cuda
powershell -File installers\fetch_engine_runtime.ps1 -EngineDir dist\engine-vulkan -Backend vulkan

gh release create v0.18.0 --title "v0.18.0 - hermes-disk-search" --target <commit> `
  --notes-file RELEASE_NOTES_v0.18.0.md dist\hds-0.18.0-windows-x64.zip `
  dist\hds-0.18.0-windows-x64.zip.sha256.txt <…остальные ассеты…>
```

Резидент держит `target\release\llm_host.exe` — перед релизной сборкой локально:
`llm_host stop` **и** `hds mcp-http stop` (или `-SkipBuild`, если бинарники уже свежие).

## 7. Грабли и запреты

- **«Отказано в доступе (os error 5)» при релизной сборке** — запущен резидент
  (`llm_host.exe`), его exe залочен. Остановить резидент/MCP или собирать `-SkipBuild`.
- **`sidecar` собирается из `hds/*.py`** (`installers/build_sidecar.ps1`): нет модулей —
  сборка падает с «missing module». Локальный `-SidecarDir sidecar` кладёт в архив
  *исходники* воркера без портативного Python — годится только для отладки, для релиза
  нужен собранный sidecar (`-WithSidecar` или артефакт CI).
- **`.venv` без `pyvenv.cfg` — не интерпретатор** (лаунчер падает с «failed to locate
  pyvenv.cfg»): такой venv пропускается, тест mock-воркера скипается, sidecar в dev-режиме
  не поднимется. РЕГРЕССИЯ 13.10.2026 (см. `crates/hds-extract/src/worker.rs`).
- **Клиентский пакет собирается вместе с серверной поставкой** (`build_rust_release*`
  зовёт `build_client_release*` с `-SkipBuild`): он и ассет релиза, и содержимое
  `<корень>/client/` (раздача сервером по `<mcp-path>/client-package.zip`). Версию
  задаёт `-Version`/тег — держите её равной `__version__` (джоба `check-version`).
- **Рантайм движка собирается только с явным `-Backend`**: на раннере без NVIDIA режим
  `auto` выбирает Vulkan — так в v0.17.0 в `hds-engine-runtime-windows-x64-cuda.zip` лежал
  Vulkan-рантайм (~86 МБ вместо ~570 МБ). Джоба `fetch-engine-runtime-windows` идёт
  матрицей `cuda`/`vulkan` именно поэтому.
- **Не выпускать с красными тестами** и не «глушить» мигающие тесты: `test-rust`
  блокирует выпуск, а мигание почти всегда означает гонку/тайминг в продукте.
- **Кросс-платформенность обязательна**: блокирующая джоба `test-rust` идёт на ubuntu,
  mac-джоба собирает под macOS. Платформенно-зависимую ветку (`#[cfg(windows)]`,
  `#[cfg(target_os = "macos")]`) нельзя «проверить только на Windows»: вызов
  windows-only функции из общего кода (`E0425`) и перемещение значения в mac-ветке
  (`E0382`) вылезают именно там. Не уверены — не гейтите вызов условной компиляцией,
  а дайте явную реализацию под `#[cfg(not(windows))]`.
- **Тесты не должны опираться на блокировки внутри одного процесса**: SQLite в одном
  процессе отдаёт `SQLITE_BUSY` без вызова busy-handler (защита от deadlock) — тест,
  который «держит слот блокировкой БД», на Linux становится недетерминированным
  (пример: держатель вынесен в отдельный процесс, см. `queue_socket.rs`).
- **Идемпотентность**: ассеты не перезаписывать руками — у `release` есть
  `gh release upload --clobber`; повторный запуск workflow безопасен.
- **Заметки на русском**, разделы: новые функции, исправления, требования к обновлению
  (например «после смены модели запустите `index --full`»). Рядом со списком ассетов —
  таблица **«Файл / Нужен? / Что это»**:
  * обязателен только `hds-<ver>-windows-x64.zip` (macOS — `hds-<ver>-macos-arm64.zip`),
    проверка sha256 — по желанию;
  * `hds-engine-runtime-windows-x64-{cuda,vulkan}.zip` и `...-macos-arm64-metal.zip` —
    только для **офлайн-установки** (обычный установщик качает рантайм движка сам по
    `runtime-manifests/engine-manifest.json`, выбирая CUDA или Vulkan по машине);
  * модели (LLM/whisper/CLIP ONNX) и sidecar в архив не входят — ставятся при установке;
  * архивы исходников рядовому пользователю не нужны.
  README остаётся каноническим источником этой таблицы; в заметках — та же таблица, чтобы
  не искать её по документации.
- **Workflow никогда не удаляет** прежние релизы и теги.
- **macOS-подпись — ad-hoc на целевой машине**: `install_macos.command` снимает карантин
  (`xattr -dr com.apple.quarantine` с архива и рантайма движка) и ставит ad-hoc подпись на
  `.app`-обёртку `~/Applications/HermesDiskSearchIndex.app` (`codesign --force --deep -s -`).
  Нотации/Developer ID нет; подписи не коммитятся (их нельзя положить в архив).
- **Откат**: `git revert` + повторный запуск workflow с новым PATCH.


