# ПЛАН: переход на общий llama-рантайм (llama-server) — hermes-disk-search

> **УСТАРЕЛ (03.10.2026).** Части про **anonymizer_proxy** (`§0` «для обоих проектов»,
> `§6` синхронизация, SYNC-COPY-пары) **неактуальны**: anonymizer_proxy отменён —
> анонимизация будет реализована **в этом проекте на Rust**. Актуальным остаётся
> **место моделей/движка**: `%LOCALAPPDATA%\llama-runtime\` и резолв `shared:<role>`
> через `current.json` — это дефолт кода (`hds-llama/src/config.rs`, `runtime.rs`)
> и то, куда кладёт установщик (`installers/fetch_llm_models.ps1`,
> `installers/fetch_engine_runtime.ps1`). Синхронизацию с proxy (§6) больше не ведём.

**Статус:** ВЫПОЛНЕНО — код (§2) + доводка и приёмка (§3.A, 2026-09-25) +
macOS (§3.C, 2026-09-25); опциональным остаётся §3.D (развитие).
**Дата сверки с кодом:** 2026-09-25.
**Родственный план:** `PLAN_SHARED_LLAMA_RUNTIME.md` в anonymizer_proxy —
планы выполнялись синхронно (см. §6; **план отменён**).

## 0. Цель

Один llama-server и один набор GGUF-моделей на машину для обоих проектов:

- одна сборка llama.cpp (cuda|vulkan) с комплектом DLL — без дублирования и
  расхождений по вариантам/версиям между проектами;
- одни и те же GGUF-модели (в т.ч. общая чат-модель) лежат в одном каталоге;
- смена активной чат-модели — одной командой/кнопкой, видна обоим проектам
  сразу (их llama-инстансы перезапускаются автоматически);
- установка/переустановка любого проекта идемпотентна: второй проект ничего
  не докачивает, если компоненты уже на месте.

Контекст проекта: три роли llama-server — chat (:8010), embedding (:8011,
`--embedding --pooling cls`), rerank (:8012, `--reranking --pooling rank`);
менеджер `hds/llama_server.py`, UI :8765, установщик `setup.ps1`.

## 1. Архитектура («Вариант C»)

Каталог: общий для всех проектов машины —
`%LOCALAPPDATA%\llama-runtime` (Windows), `~/Library/Application Support/llama-runtime`
(macOS) или `~/.local/share/llama-runtime` (прочие ОС); переопределяется
`LLAMA_RUNTIME_DIR`. Общий с anonymizer_proxy:

```
llama-runtime\
  bin\                          llama-server.exe + DLL (одна сборка cuda|vulkan)
  models\chat\                  общая чат-модель + current.json
  models\embedding\             bge-m3-Q8_0.gguf
  models\rerank\                bge-reranker-v2-m3-q8_0.gguf
  projects.json                 реестр проектов (hermes + anonymizer_proxy)
  version.json                  вариант сборки (cuda|vulkan) + тег llama.cpp
```

Конфигурация проекта (`config.yaml`):

```yaml
llm_server:
  bin: ""                                   # пусто: общий рантайм > tools/llama.cpp/ > PATH
  chat:      { model: "shared:chat",      port: 8010 }
  embedding: { model: "shared:embedding", port: 8011 }
  rerank:    { model: "shared:rerank",    port: 8012 }
```

`shared:<role>` резолвится в `hds/llama_server.py::_abs_model` через общий
модуль `hds/llama_runtime.py` (SYNC-COPY с
`anonymizer_proxy/llama_runtime.py`); при отсутствии рантайма роль понятно
падает с ошибкой пути.

## 2. Выполнено (сверено с кодом)

- [x] `hds/llama_runtime.py` — SYNC-COPY общего модуля;
- [x] `installers/ensure_llama_runtime.ps1` — SYNC-COPY установщика рантайма
      (бинарь нужного варианта + CUDA cudart, GGUF-пресеты, манифест,
      регистрация проекта);
- [x] миграция машины: `tools\llama.cpp` (CUDA-сборка) → `llama-runtime\bin`;
      `models\chat\qwen3.5-9b-Q6_K.gguf` → `models\chat\Qwen3.5-9B-Q6_K.gguf`;
      `models\embedding\bge-m3-Q8_0.gguf` → рантайм; докачан реранкер
      `bge-reranker-v2-m3-q8_0.gguf`; `version.json` (cuda), `current.json`
      (активная `Qwen3.5-9B-Q6_K.gguf`), `projects.json` (оба проекта);
- [x] `hds/llama_server.py`: `find_binary` — `llm_server.bin → общий рантайм →
      tools\llama.cpp → PATH`; `_abs_model` резолвит `shared:<role>`;
      обновлены тексты ошибок;
- [x] `config.yaml` и дефолты `hds/config.py`: `shared:chat|embedding|rerank`;
- [x] `hds/ui_server.py`: `GET /api/chat-model`, `POST /api/chat-model/set`
      (фоновая смена с прогрессом, перезапуск chat-инстанса проекта и
      инстансов из `projects.json`), `_gguf_path()` через рантайм;
- [x] `assets/ui.html`: виджет «Общая чат-модель» в карточке «LLM-серверы»
      (селект файлов+пресетов, «Применить и перезапустить», поллинг);
- [x] `setup.ps1`: llama-блок и модели → вызов
      `installers\ensure_llama_runtime.ps1 -Models chat,embedding,rerank
      -ProjectName hermes-disk-search -RestartArgs "-m hds.llama_server restart chat"`;
      вызов `ensure_models.ps1` убран (скрипт задепрекейчен);
- [x] живая проверка: chat (8010) и embedding (8011) работают из общего
      рантайма (`state=llama`, `model_ready=true`, бинарь из
      `llama-runtime\bin`); оба проекта видны в `projects.json`;
- [x] очистка: пустые `models\chat`, `models\embedding`, `tools\llama.cpp`
      удалены;
- [x] **macOS-шаг (§3.C)**: каталог рантайма на darwin —
      `~/Library/Application Support/llama-runtime` (в `llama_runtime.py`,
      обе копии); новый `installers/ensure_llama_runtime.sh` — бинарь из
      Homebrew ссылкой в `bin/` либо пре-билд llama.cpp с GitHub Releases
      (+снятие карантина Gatekeeper), модели chat/embedding/rerank,
      `version.json`, манифест, регистрация проекта; `install_macos.command`
      вызывает его (шаг 6.1); legacy `installers/ensure_models.sh` удалён;
      кросс-платформенные подсказки об установщике — `install_hint()` в
      `llama_runtime.py` и `_ensure_hint()` в `hds/llama_server.py`;
- [x] тесты: `tests/test_llama_runtime.py` — пути рантайма по ОС (win32/
      darwin/др.), симлинк-бинарь (macOS/Homebrew), манифест и
      `resolve_model`, реестр/`switch`, обзор для UI, интеграция с
      `hds.llama_server` (текст ошибки с установщиком текущей ОС);
      прогоняется и в macOS-джобе CI.

## 3. Ход выполнения

### A. Доводка (обязательно) — ВЫПОЛНЕНО 2026-09-25

1. **UI hermes перезапущен** (8765): старый процесс (pid 17592) остановлен,
   поднят новый (`pythonw -m hds.cli ui --port 8765 --no-browser`, pid 9852).
   Проверено живым запросом: `GET /api/chat-model` → 200,
   `current = Qwen3.5-9B-Q6_K.gguf`, `binary_ok = true`, проекты
   `hermes-disk-search` + `anonymizer_proxy`, `server.state = llama`,
   `server.model` = `…\llama-runtime\models\chat\Qwen3.5-9B-Q6_K.gguf`.
2. **Документация обновлена**:
   - `README.md`: новый раздел «Общий llama-рантайм и смена чат-модели»
     (раскладка рантайма, `shared:<role>`, смена модели из UI и через
     `python -m hds.llama_runtime switch <файл>`, установщик
     `ensure_llama_runtime.ps1`), правки шага установки, таблицы `llm_server`
     и раздела `rerank` (в т.ч. требование batch-флагов), таблицы компонентов;
     оговорки по macOS (обновлены в §3.C) и по `models/` проекта (там остаются
     только Whisper-модели);
   - `PLAN_LLAMA_SERVER.md`: врезка «Актуализация» (общий рантайм вместо
     `models/` проекта и `ensure_models.*`), пример `llm_server` в §6 приведён
     к `shared:<role>` + batch-флаги rerank;
   - подсказки в коде (`hds/diag.py`, `hds/embedder.py`, `hds/llama_server.py`)
     переведены на `installers/ensure_llama_runtime.ps1`.
3. **Решение по `ensure_models`** (зафиксировано в README):
   fallback-копирование уже скачанных GGUF из `~/.lmstudio` перенесено в общий
   `installers/ensure_llama_runtime.ps1` (SYNC-COPY с
   `anonymizer_proxy/scripts/ensure_llama_runtime.ps1`; хэши совпадают),
   сам `installers/ensure_models.ps1` **удалён**; `installers/ensure_models.sh`
   тогда оставался legacy для macOS — **удалён вместе с macOS-шагом (§3.C)**.
   Проверено: `ensure_llama_runtime.ps1 -Models chat,embedding,rerank`
   отрабатывает идемпотентно (бинарь cuda + 3 модели «уже на месте», ничего
   не качает).
4. **Реранк проверен — найдена и исправлена ошибка**: rerank-роль поднята из
   общего рантайма (`shared:rerank` → `llama-runtime\models\rerank\bge-reranker-v2-m3-q8_0.gguf`),
   `rerank.enabled: true`. Первый же прогон `ask` дал 500 от llama-server:
   `input (700 tokens) is too large to process. increase the physical batch
   size (current batch size: 512)`. Исправление: в `llm_server.rerank.extra_args`
   добавлены `--batch-size 8192 --ubatch-size 8192` (как у роли `embedding`) —
   в живой `config.yaml` и в дефолте `hds/config.py`. После перезапуска роли
   `ask` отработал штатно (ответ 1118 символов, в stderr нет
   `[rerank] недоступен`, в логе — успешная обработка фрагментов до 638
   токенов); прямой запрос к `/v1/rerank` — HTTP 200, латентность 0.10 с,
   порядок релевантности корректен.

### B. Приёмка (команды)

```powershell
Set-Location C:\Users\Sasha\hermes-disk-search
.venv\Scripts\python.exe -m hds.llama_server status all     # 3 роли, model_ready
.venv\Scripts\python.exe -m hds.llama_runtime list          # current/binary/projects
```

Ручной сценарий (осталось за оператором): UI «LLM-серверы» → выбрать модель →
«Применить и перезапустить» → отчёт `anonymizer_proxy: ok · hermes-disk-search: ok`;
затем `python -m hds.llama_server status chat` показывает новый `model` из
манифеста.

### C. macOS — ВЫПОЛНЕНО 2026-09-25

Что сделано (общее с планом proxy, но реализация пока в hermes):

1. **Каталог рантайма на darwin** — `~/Library/Application Support/llama-runtime`
   (`runtime_dir()` в `llama_runtime.py`, обе копии; остальные ОС — как были:
   `%LOCALAPPDATA%` / XDG). `LLAMA_RUNTIME_DIR` по-прежнему переопределяет.
2. **`installers/ensure_llama_runtime.sh`** (bash 3.2+, LF, `bash -n` — чисто) —
   macOS/Linux-аналог .ps1 с тем же набором шагов и теми же URL моделей:
   - бинарь: системный `llama-server` (PATH, `/opt/homebrew/bin`,
     `/usr/local/bin`, `/opt/local/bin`) → `brew install llama.cpp` при его
     отсутствии → пре-билд llama.cpp с GitHub Releases (`-bin-macos-arm64|x64`,
     через `python3` + `curl` + `unzip`) со снятием `com.apple.quarantine`;
     в `bin/` кладётся **символическая ссылка** — обновление llama.cpp через
     brew подхватывается автоматически (`find_binary()` разыменовывает ссылку);
   - `version.json` (`variant: metal|cpu`, `source`, `installed_at`);
   - модели `chat`/`embedding`/`rerank` идемпотентно (+копирование из
     `~/.lmstudio/models`, поддержка `HF_TOKEN`), манифест `current.json`,
     регистрация проекта в `projects.json` (через `python3`);
   - опции: `--models`, `--project-name`, `--project-root`, `--restart-args`,
     `--runtime-dir`, `--force`, `--help`.
3. **`installers/install_macos.command`**: шаг 5 (brew llama.cpp) дополнен
   пояснением, шаг 6.1 — вызов `ensure_llama_runtime.sh` с регистрацией проекта
   (`-m hds.llama_server restart chat`); `installers/ensure_models.sh` удалён.
4. **Кросс-платформенные подсказки**: `llama_runtime.install_hint()` и
   `llama_server._ensure_hint(role)` — в текстах ошибок (`resolve_model`,
   `build_command`), `hds/diag.py`, `hds/embedder.py` фигурирует установщик
   текущей ОС (`.ps1` на Windows, `.sh` на macOS).
5. **Тесты**: `tests/test_llama_runtime.py` (18 тестов; 2 симлинк-теста
   платформенно-зависимы) — пути по ОС, `install_hint`, бинарь/ссылка,
   манифест, `resolve_model`, реестр, `switch_chat_model`, обзор для UI,
   интеграция с менеджером; файл исполняется и в macOS-джобе CI
   (`test-macos`). Полный прогон: 209 тестов, OK.
6. **Проверено локально**: `bash -n` скрипта (git-bash), smoke-прогон
   `ensure_llama_runtime.sh --runtime-dir <tmp>` (идемпотентность, манифест,
   `--help`, неизвестная роль), регрессия proxy `test_llama_runtime` (5/5 —
   общий модуль после правки darwin-пути), `compileall`.

Осталось в proxy (его §3.C): `scripts/ensure_llama_runtime.sh` (копия файла
из hermes), macOS-ветки `install.sh` / `install_launchagent.sh`. До этого
proxy на macOS деградирует как и раньше: бинарь ищется в PATH (Homebrew), а
`shared:chat` без манифеста даёт понятную ошибку пути — блокирующей проблемы
нет, но полный паритет требует переноса .sh.

### D. Развитие (опционально)

- переключение embedding-модели тем же механизмом: `shared:embedding` уже
  поддержан резолвером — нужны только список файлов и пресеты в UI;
- пресеты чат-моделей — синхронно с планом proxy (см. §6).

## 4. Откат / escape-hatch

- `llm_server.bin` — явный путь к бинарю мимо рантайма;
- `llm_server.<role>.model` — абсолютный путь к GGUF мимо манифеста
  (приоритетнее `shared:<role>`);
- `LLAMA_RUNTIME_DIR` — перенести рантайм;
- полный откат миграции: вернуть файлы в `tools\llama.cpp` и
  `models\{chat,embedding}`, вернуть прежние пути в `config.yaml`;
- macOS: откат без правки кода — `llm_server.bin` на бинарь Homebrew
  (`/opt/homebrew/bin/llama-server`) и явные пути к GGUF в
  `llm_server.<role>.model` (рантайм при этом можно просто не использовать).

## 5. Риски

- рерангер скачан заново (~600 МБ) — при включённом реранке сверить качество;
- смена чат-модели прерывает генерацию chat-инстанса (диалог подтверждения в
  UI);
- старый UI-процесс (8765) до рестарта работает на старом коде и не знает
  новых эндпоинтов;
- ручной запуск llama-server мимо `hds.llama_server` не попадает в
  перезапуск при смене модели;
- macOS: `bin/llama-server` — ссылка на сборку Homebrew, поэтому
  `brew upgrade llama.cpp` меняет фактическую версию движка (флаги
  `llm_server.*.extra_args` могут потребовать правки: например
  `--cache-type-k/v q8_0` требует flash-attention — добавьте `-fa on`, а на
  Intel-маках без Metal-GPU — `-ngl 0`);
- macOS: пре-билд (путь без brew) распаковывается в `bin/` со снятием
  карантина; при ручном обновлении файлов из архива карантин вернётся —
  `xattr -dr com.apple.quarantine <рантайм>/bin`.

## 6. Синхронизация с планом anonymizer_proxy

- **SYNC-COPY-пары** (правятся всегда вместе, содержимое идентично):
  `hds/llama_runtime.py` ↔ `anonymizer_proxy/llama_runtime.py`;
  `installers/ensure_llama_runtime.ps1` ↔ `scripts/ensure_llama_runtime.ps1`;
  `installers/ensure_llama_runtime.sh` ↔ `scripts/ensure_llama_runtime.sh`
  (создан в hermes; копия в proxy появится вместе с его mac-фазой — до этого
  в proxy .sh отсутствует, и это отражено в его §3.C).
  Проверка: совпадение `Get-FileHash` всех пар.
- **Порядок при изменениях**: сначала общее (модуль + ensure-скрипт +
  пресеты в обеих парах), затем правки проектов, затем перезапуски
  (hermes UI :8765 → proxy `start_proxy.cmd`).
- **Реестр `projects.json`** — предусловие синхронной смены модели: перед
  первой сменой убедиться, что оба проекта зарегистрированы
  (`python -m hds.llama_runtime list`).
- **Пресеты моделей** правятся в четырёх местах (две копии
  `llama_runtime.py` + две копии ensure-скрипта) плюс `DEFAULT_CHAT`.
- **Версии/релизы**: изменения общего рантайма выпускать синхронно с proxy,
  тег ставить только при зелёных тестах обоих проектов.
- **Новая машина**: порядок установки любой (ensure идемпотентен); первый
  установщик создаёт рантайм и chat-модель, второй докачивает свои роли
  (embedding/rerank).

