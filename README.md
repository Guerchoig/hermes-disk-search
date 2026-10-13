# hermes-disk-search

Локальный поиск и «спроси по своим файлам» — по дискам машины. **Ядро на Rust**:
индексация, гибридный поиск (FTS5 + вектор + CLIP), RAG-ответы, MCP-сервер и
веб-интерфейс. Python остаётся **только там, где он нужен по делу** — извлечение текста
из офисных форматов/PDF/OCR и русская лемматизация (автономный воркер `sidecar`).

> Публичный проект (`Guerchoig/hermes-disk-search`). **Установка:** скачайте архив со
> [страницы релизов](https://github.com/Guerchoig/hermes-disk-search/releases/latest) и следуйте
> разделу «Установка на Windows: по шагам» (или «Установка — macOS»). Состояние и журналы волн
> миграции: `STATUS.md`, `tools/parity/W4_REPORT.md`, `MIGRATION_PLAN_RUST.md`.

## Карта документов

| Документ                                                                  | Что внутри                                                                                                                                                                                                                                                                                                                                                                                                         | Когда читать                                                                               |
| --------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| **`README.md`** (этот файл)                                       | продукт и старт: требования, установка (Windows/macOS), архивы релиза, CLI и «Справочник команд», веб-UI, автотранскрибация, MCP, интеграции Hermes/Cline, llm-host, sidecar, БД и файлы-сигналы,`config.yaml`, разработка, диагностика                                                  | первое знакомство и повседневная работа                            |
| **`README_SERVER_MULTIUSER.md`**                                          | серверная многопользовательская подсистема (M1–M9): роли, сценарии A–F, портрет отказов, эксплуатация по этапам (§8a–§8e), десктоп-режим, шпаргалка команд и HTTP-API, сетевой периметр и конкурентность —**единый источник по серверу** | админ/эксплуатация сервера, подключение пользователей |
| **`installers/client_readme.md`**                                         | тонкий клиент (лежит как`README.md` в клиентском архиве)                                                                                                                                                                                                                                                                                                                             | подключение ноутбука к серверу                                             |
| **`STATUS.md`**                                                           | журнал волн миграции и текущие статусы                                                                                                                                                                                                                                                                                                                                                     | «что уже сделано»                                                                      |
| **`PLAN_*.md`**, `PLAN_SERVER_MULTIUSER.md`, `MIGRATION_PLAN_RUST.md` | планы, дорожные карты и промты передачи контекста (не переписываются)                                                                                                                                                                                                                                                                                             | контекст для разработки                                                          |
| **`RELEASE_NOTES_*.md`**                                                  | что нового в конкретной версии                                                                                                                                                                                                                                                                                                                                                                    | обновление/релиз                                                                       |
| **`tools/parity/*_REPORT.md`**, `tools/parity/out/*`                    | отчёты волн/этапов и артефакты (`out/` в git не идёт)                                                                                                                                                                                                                                                                                                                                    | разбор замеров и приёмки                                                         |

## Как это работает

```
        llm-host.exe  (резидент, владелец GPU)             :8010 chat
        движок openresearchtools (LLM + ASR)               :8011 embedding
              │  OpenAI-совместимый фасад + /internal/*     :8012 rerank
              │
  hds.exe ────┤  index / search / ask / status / check / ui (:8765)
  hds_mcp.exe │  MCP stdio
  hds mcp     ┘  --http -> :8787     ┌────────────────────────────────┐
                                     │ sidecar (Python-воркер, stdio)  │
                                     │  extract / normalize (.mpp,     │
                                     │  pdf/docx/xlsx/pptx/OCR)        │
                                     └────────────────────────────────┘
```

* **Индексация** (`hds index` / watcher / UI): обход диска → извлечение текста →
  чанки → эмбеддинги (`:8011`) → SQLite (`index.db`) + FTS5 + `sqlite-vec` (+ опц. CLIP).
* **Поиск** (`hds search`): RRF-слияние FTS5 и векторного KNN; опц. CLIP по картинкам.
* **RAG** (`hds ask`, MCP `ask_my_files`): гибридный поиск → опц. реранк (`:8012`) →
  ответ чат-моделью (`:8010`) со ссылками `[N]`.
* **llm-host** — единый резидент: держит роли чат/эмбеддинги/реранк на GPU и **сам ASR**
  (движок вместо llama.cpp), раздаёт OpenAI-совместимый HTTP на 8010–8012 и владеет
  VRAM-диспетчером. Кросс-процессной адресации инстансов у движка нет — поэтому всё
  общение идёт через этот фасад.
* **sidecar** — автономный Python-процесс (stdio + JSON-RPC 2.0) для того, что решено
  оставить в Python: офисные форматы/PDF/OCR и `pymorphy3`-лемматизация FTS. Ядро
  запускает его лениво и владеет процессом. `.mpp` (MS Project) требует Java 11+ (см. ниже).

## Требования (Windows x64)

* **Windows 10/11 x64** — основная (блокирующая) цель релиза.
* **macOS 14+ (Apple Silicon)** — собирается и **публикуется best-effort**, но на реальной
  машине **не проверен** (`MIGRATION_PLAN_RUST.md` §10.0); установка — `install_macos.command`.
* **GPU** — не обязательна: без CUDA-карты движок работает на Vulkan/CPU (медленнее).
  Рекомендуется NVIDIA ≥8 ГБ VRAM (проверено на RTX 3060 12 ГБ: чат 9B + эмбеддинги + ASR).
* **Microsoft Visual C++ Redistributable (x64)** — установщик поставит при отсутствии.
* **Java 11+** — только для `.mpp` (MS Project). Без неё `.mpp` индексируется с пометкой,
  остальное не затрагивается. Воркер сам находит `%LOCALAPPDATA%\jdk-21\*\bin\server\jvm.dll`
  (`JAVA_HOME` не обязателен).
* **ffmpeg** — для транскрипции медиа (движок ASR); `winget install Gyan.FFmpeg` (ставит установщик).
* **Tesseract OCR** — по желанию (текст на картинках/сканах); ставится по согласию.
* Сеть — для разовой загрузки: рантайм движка (по манифесту), GGUF-модели, модели CLIP.

## Установка на Windows: по шагам (от скачивания до первого поиска)

Порядок для **обычного пользователя**: скачали два файла → распаковали → запустили
`setup.cmd` → ответили на 5 вопросов → задали корни и запустили индексацию.

**Что понадобится:** Windows 10/11 x64, ~12 ГБ свободного места (движок + модели) плюс место
под индекс, интернет для разовых загрузок. **GPU не обязателен**: без NVIDIA-карты движок
работает на Vulkan/CPU (медленнее). Права администратора не нужны; для автозапуска
установщик спросит согласие (создаёт задачи Планировщика).

### Шаг 1. Скачайте файлы со страницы релиза

Откройте `https://github.com/Guerchoig/hermes-disk-search/releases/latest` и скачайте:

| Файл                                                           | Нужен?             | Что это                                                                                                                                                                                                                                          |
| ------------------------------------------------------------------ | ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `hds-<версия>-windows-x64.zip` (~210 МБ)                 | **да**          | само приложение:`bin\` (`hds.exe`, `hds_mcp.exe`, `llm_host.exe`), установщик `setup.cmd`, воркер извлечения `sidecar\`, манифесты, скрипты, ярлыки, `config.example.yaml` |
| `hds-<версия>-windows-x64.zip.sha256.txt`                  | по желанию     | контрольная сумма архива (шаг 2)                                                                                                                                                                                              |
| `hds-engine-runtime-windows-x64-cuda.zip` (~570 МБ)             | нет\*                | рантайм движка (LLM + ASR) под **NVIDIA/CUDA**; для машин без NVIDIA есть `hds-engine-runtime-windows-x64-vulkan.zip` (~90 МБ, тот же рантайм под Vulkan).\*Установщик качает рантайм сам по манифесту из upstream; ассеты нужны только для установки **без интернета** (шаг 5′)   |
| `hds-<версия>-macos-arm64.zip`                             | только для Mac | версия для Apple Silicon (см. «Установка — macOS»)                                                                                                                                                                              |
| `hermes-disk-search-<версия>-windows.zip` / `-macos.zip` | нет                  | архивы исходников (для сборки из кода)
| `hds-client-<версия>-windows-x64.zip` (+ `.sha256.txt`) | для тонкого клиента | клиент для ноутбука (Cline без Hermes, M9); **тот же файл** сервер отдаёт по `<ServerUrl>/mcp/client-package.zip` (см. «Клиент (ноутбук)»)                                                                                                                                                                                  |

> **Модели в релиз не входят.** Установщик докачает их с HuggingFace: чат Qwen3.5-9B,
> эмбеддинги bge-m3, реранкер bge-reranker — вместе ≈9 ГБ (плюс, по желанию, whisper ≈1.5 ГБ
> и CLIP ≈850 МБ). Они ложатся в общий рантайм машины `%LOCALAPPDATA%\llama-runtime\models\`.

### Шаг 2 (необязательно). Проверьте, что архив скачался целиком

Подставьте свою версию вместо `0.14.0`:

```powershell
Get-FileHash .\hds-0.14.0-windows-x64.zip -Algorithm SHA256   # хеш архива
Get-Content .\hds-0.14.0-windows-x64.zip.sha256.txt           # ожидаемый хеш из релиза
```

Строка из файла `.sha256.txt` начинается с ожидаемого хеша — сверьте с выводом первой команды.

### Шаг 3. Распакуйте архив в локальную папку

Например `D:\hds` (ПКМ по zip → «Извлечь всё…»). Подойдёт любая папка на диске; правами
администратора ничего устанавливать не нужно. **Не распаковывайте поверх уже работающей
установки** — Windows не даёт заменить запущенный `hds.exe`; для обновления есть `update.ps1`
(см. «Установка с версионированием»).

### Шаг 4. Запустите `setup.cmd` (двойной щелчок)

Скрипт снимет пометку «скачано из интернета» (иначе политика PowerShell попросит подпись) и
запустит `setup.ps1`. Он задаст несколько вопросов — можно просто нажимать Enter (значения по
умолчанию указаны в скобках):

| Вопрос установщика                                     | Что это даёт                                                                                              |
| ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `Install Tesseract OCR … [y/N]`                                      | текст на картинках и сканах                                                                  |
| `Download the whisper model now (about 1.5 GB)? [y/N]`                | расшифровка речи в видео/аудио                                                            |
| `Download the CLIP models now (about 850 MB)? [y/N]`                  | поиск картинок по содержанию                                                               |
| `Download the Sortformer diarization model now (about 449 MB)? [y/N]` | автотранскрибация: разметка реплик по спикерам                             |
| `Set up autostart at logon (watcher + MCP + llm-host)? [Y/n]`         | содержимое индекса, MCP-сервер и llm-host поднимаются сами при входе |
| `Start the web UI automatically at logon (port 8765)? [y/N]`          | веб-интерфейс всегда под рукой (иначе — по ярлыку)                          |

Что установщик делает за вас: runtime Visual C++, `ffmpeg`, рантайм движка (≈90–200 МБ,
проверка sha256), GGUF-модели чат/эмбеддинги/реранк (≈9 ГБ), `config.yaml` из образца, задачи
автозапуска, интеграции Hermes/Cline, ярлык «Hermes Disk Search» и диагностику `hds check`.
Первый прогон занимает **10–40 минут** (почти всё время — скачивание моделей).

### Шаг 5. Проверьте результат

```powershell
cd D:\hds
.\bin\hds.exe check
```

Ожидаемый вид: `[ok]` для базы данных, корней индексации, чата, эмбеддингов, OCR/ffmpeg/
pymorphy3/mpxj, реранкера и GPU; строка про whisper/Vulkan помечена как «проверяется вручную» —
это норма. Если чат/эмбеддинги помечены как недоступные — значит не запущен `llm_host.exe`
(шаг 6).

### Шаг 5′. Установка без интернета (офлайн)

Скачайте на другой машине `hds-<версия>-windows-x64.zip` **и** рантайм движка под свою
видеокарту — `hds-engine-runtime-windows-x64-cuda.zip` (NVIDIA) **или**
`hds-engine-runtime-windows-x64-vulkan.zip` (AMD/Intel, без NVIDIA), распакуйте приложение (шаг 3), затем положите рантайм
движка туда, где его ищет `llm-host`, и запустите установщик с `-SkipEngine`:

```powershell
# распаковать рантайм движка туда, где его ищет llm-host
$engine = Join-Path $env:APPDATA 'OpenResearchTools\TranscribeOffline\Engine'
New-Item -ItemType Directory -Force $engine | Out-Null
Expand-Archive .\hds-engine-runtime-windows-x64-cuda.zip -DestinationPath $engine -Force
# без NVIDIA — тот же путь, но архив hds-engine-runtime-windows-x64-vulkan.zip

# поставить всё, кроме скачиваемого движка и моделей
powershell -NoProfile -ExecutionPolicy Bypass -File .\setup.ps1 -SkipEngine -SkipModels
```

Модели GGUF можно принести на флешке и разложить вручную в
`%LOCALAPPDATA%\llama-runtime\models\<роль>\` (`chat\`, `embedding\`, `rerank\`); для чата
дополнительно нужен `chat\current.json` с именем файла — проще всего запустить
`installers\fetch_llm_models.ps1` там, где интернет есть.

### Шаг 6. Резидент `llm-host` (если отказались от автозапуска)

`llm-host.exe` держит модели на портах 8010–8012 и владеет GPU. Запустить в отдельном окне:

```powershell
.\bin\llm_host.exe run
```

Или поставить автозапуск: `powershell -File installers\install_llm_host_task.ps1 -Start`.
Модели загружаются лениво — первый ответ после старта будет с задержкой.

### Шаг 7. Задайте, что индексировать, и запустите индексацию

Корни индексации берутся из `config.yaml` (`index.roots`; в образце — `D:\`). Задать их удобнее
всего в веб-интерфейсе (шаг 8), но можно сразу запустить ярлык **«Индексация дисков»** на рабочем
столе (или `.\run_index.ps1`) — тогда индексируется то, что уже прописано в конфиге.

Первичная индексация диска с сотнями тысяч файлов идёт **часами** (извлечение текста, OCR,
эмбеддинги) — её принято запускать «на ночь». Прогресс виден в веб-интерфейсе; прервать можно
кнопкой «Стоп» или файлом `index.stop`. Повторные запуски докачивают только изменения
(инкрементально), а watcher следит за файлами в фоне.

### Шаг 8. Откройте веб-интерфейс и доведите настройку

Ярлык **«Hermes Disk Search»** на рабочем столе (или `.\run_ui.ps1`) → откроется
`http://127.0.0.1:8765`. Порядок действий:

1. **Настройки (config.yaml)** → корни (например `D:\`) → **Сохранить корни**;
2. там же — исключения (мусорные каталоги) → **Сохранить исключения**;
3. **Состояние** → **Старт** (или **Полная** для переиндексации с нуля); **Пауза/Продолжить** —
   чтобы временно не нагружать диск;
4. **Модель чата** — выбор GGUF из общего рантайма (`models\\chat`) и аккуратный перезапуск
   llm-host; **MCP-сервер** — статус и запуск/остановка общего инстанса для агентов;
   **Настройки Cline** → «Синхронизировать настройки Cline» — если пользуетесь агентом Cline.
   Поиск и «вопрос по файлам» в интерфейсе не дублируются: для запросов есть агенты,
   MCP-инструменты и CLI (`hds search`, `hds ask`).

Если интерфейс не открылся — смотрите журнал `%LOCALAPPDATA%\hermes-disk-search\ui.err.log`.

### Шаг 9. Подключение агента (Hermes Desktop / Cline) — по желанию

Установщик уже выполнил `install_hermes.ps1` и `install_cline.ps1`, если эти программы есть на
машине. Поставили их **позже** — просто повторите:

```powershell
cd D:\hds
.\install_hermes.ps1
.\install_cline.ps1
```

После подключения к Cline перезапустите приложение: он читает настройки моделей и MCP при старте.
Синхронизировать всё в любой момент — кнопка «Синхронизировать настройки Cline» в веб-интерфейсе
или команда `.\bin\hds.exe cline-sync`.

### Шаг 10. Обновление и удаление

* **Обновление** — только отдельным процессом: `installers\update.ps1`
  (см. «Установка с версионированием» ниже). Он сам остановит задачи и резидента, поставит новую
  версию и запустит всё обратно; прежние версии сохраняются.
* **Удаление** — удалите папку с программой и, при желании, данные:
  `%LOCALAPPDATA%\llama-runtime` (модели ≈9 ГБ), `%APPDATA%\OpenResearchTools` (движок),
  файл индекса `index.db`, задачи Планировщика `HermesDiskSearch*` (или ярлыки в
  `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup`).

### Если что-то не получилось

| Симптом                                                                                                 | Причина и решение                                                                                                                                             |
| -------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| SmartScreen: «Windows protected your PC» при запуске`hds.exe`                                    | сборка не подписана: «More info» → «Run anyway»                                                                                                        |
| PowerShell: «выполнение сценариев отключено / требуется подпись» | запускайте через`setup.cmd`, либо `powershell -NoProfile -ExecutionPolicy Bypass -File setup.ps1`                                                     |
| `hds check`: чат/эмбеддинги недоступны                                                | не запущен резидент:`.\bin\llm_host.exe run` (шаг 6)                                                                                                   |
| Индексация стоит на месте                                                                | стоит`index.pause` (кнопка «Продолжить» в UI) или `index.stop`; проверьте «Состояние»                                     |
| В UI «Tesseract OCR не найден»                                                                      | включите OCR и запустите`installers\configure_ocr.py` (нужен Python воркера), либо переустановите установщиком |
| `.mpp` не индексируются                                                                       | нужна Java 11+ (см. «Требования»)                                                                                                                         |
| MCP-сервер не отвечает, порт 8787 занят                                               | `.\bin\hds.exe mcp-http status`; «чужой» сервис на порту нужно освободить, затем `mcp-http restart`                              |
| ffmpeg не найден после установки                                                         | откройте**новое** окно PowerShell (PATH обновится)                                                                                           |
| Нет NVIDIA-карты                                                                                       | движок уйдёт на Vulkan/CPU: работает, но медленнее; чат-модель можно положить меньше                             |

### Подробный журнал: что именно делает `setup.ps1`

1. **Проверка артефакта** — есть ли `bin\hds.exe` (иначе понятное сообщение: это не Rust-сборка).
2. **Системные зависимости через winget** — `ffmpeg` (авто), `Tesseract OCR` (по согласию),
   Visual C++ Redistributable (если нет).
3. **Проверка sidecar-воркера** — рукопожатие `hello` (интерпретатор: `HDS_EXTRACT_PYTHON` →
   `sidecar\python` → `.venv`); при неудаче — подсказка про системный Python 3.10+.
4. **`config.yaml`** — из `config.example.yaml`, если нет; эвристика «диски из конфига отсутствуют».
5. **Рантайм движка** — `installers\fetch_engine_runtime.ps1` (по `runtime-manifests\engine-manifest.json`,
   выбор `cuda`/`vulkan`, проверка **sha256**; кладётся в `%APPDATA%\OpenResearchTools\TranscribeOffline\Engine`).
6. **Модели** — `fetch_llm_models.ps1` (GGUF chat/embedding/rerank в общий `%LOCALAPPDATA%\llama-runtime`),
   опц. `fetch_whisper_model.ps1` (ASR), опц. `fetch_clip_models.ps1` (CLIP ~850 МБ),
   опц. `fetch_diarization_model.ps1` (sortformer для автотранскрибации ~449 МБ).
7. **Задачи Планировщика** (по согласию): `HermesDiskSearchLlmHost` (`bin\llm_host.exe run`),
   `HermesDiskSearchWatch` (`bin\hds.exe watch`), `HermesDiskSearchMcp` (`bin\hds.exe mcp-http run`),
   `HermesDiskSearchTranscribe` (`bin\hds.exe transcribe-watch`), опц. `HermesDiskSearchUi`.
8. **Интеграции** — `install_hermes.ps1` / `install_cline.ps1` (MCP по URL `:8787` или stdio `bin\hds_mcp.exe`).
9. **Ярлык** на рабочем столе («Hermes Disk Search» → `run_ui.ps1`).
10. **Диагностика** — `bin\hds.exe check`.

Флаги: `-SkipModels`, `-SkipEngine`, `-NoAutostart`, `-SkipIntegrations`, `-SmokeTest`
(мини-индекс на временной БД).

### Установка с версионированием (`app\<версия>`)

Чтобы обновлять, **не перезаписывая запущенный `hds.exe`/`llm_host.exe`** (на Windows нельзя),
используйте версионную раскладку:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_app_version.ps1 `
    -From <hds-<версия>-windows-x64 | .zip> -Version <версия> -Root D:\hds -SetCurrent
```

Раскладка: `app\<версия>\` (код версии + `config.yaml`), `app\current` — junction на активную
версию, общие `data\` и `models\` (junction-ы). `db_path` должен быть абсолютным или
`data\index.db` — иначе индекс будет «на версию» (скрипт предупредит). Обновление целиком —
**отдельным** процессом (процесс не может заменить сам себя):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File installers\update.ps1 `
    -From hds-<новая>-windows-x64.zip -Version <новая> -Root D:\hds
```

`update.ps1` останавливает задачи/резидента → ставит новую версию → переключает `app\current` →
запускает задачи. Прошлые версии сохраняются — откат = вернуть `current` на прежнюю.

### Блокировки «скачано из интернета» и подпись

Бинарники не подписаны (подпись кода в бюджет не входит). Поэтому:

* `setup.cmd`/`setup.ps1` снимают MotW (`Unblock-File`) — иначе политика `RemoteSigned`
  потребует цифровую подпись для `.ps1`;
* SmartScreen может показать «Windows protected your PC» для `hds.exe` — «More info → Run anyway».

## Установка — macOS (Apple Silicon, best-effort)

1. Скачайте со страницы релиза архив `hds-<версия>-macos-arm64.zip`
   (`https://github.com/Guerchoig/hermes-disk-search/releases/latest`). Рантайм движка
   (`hds-engine-runtime-macos-arm64-metal.zip`) скачивать не нужно — `install_macos.command`
   заберёт его сам по манифесту (проверка sha256).
2. Распакуйте архив, например в `~/hds`.
3. Запустите **`install_macos.command`** (двойной щелчок; при карантине — ПКМ → «Открыть»
   либо `bash install_macos.command`).
4. Дальше — как на Windows: модели докачиваются установщиком, затем `~/.local/bin/hds check`,
   ярлыки на рабочем столе («Hermes Disk Search» — UI, «Индексация дисков» — индекс),
   корни задаются в UI `http://127.0.0.1:8765`.

`install_macos.command` (ASCII-only) делает по шагам:

1. **Карантин** — `xattr -dr com.apple.quarantine` (сборка из загрузки не подписана).
2. **Бинарники** — `bin/{hds,hds_mcp,llm_host}` → `~/.local/bin` (+ подсказка про `PATH`).
3. **`config.yaml`** — из `config.example.yaml`, если нет.
4. **Рантайм движка** — по `runtime-manifests/engine-manifest.json` (ассет `macos-arm64`/`metal`,
   проверка **sha256**; `~/Library/Application Support/OpenResearchTools/TranscribeOffline/Engine`).
5. **Ярлыки** — на рабочем столе «Hermes Disk Search» (веб-UI) и «Индексация дисков» (индекс);
   `.app`-бандл «HDS Индексация» → `~/Applications` с **ad-hoc подписью** (`codesign --force --deep -s -`).
6. **Автозапуск (LaunchAgent)** — `local.hds.watch` (`hds watch`), `local.hds.mcp`
   (`hds mcp-http run`), `local.hds.llmhost` (`llm_host run`) в `~/Library/LaunchAgents`
   (логи — `~/Library/Logs/hermes-disk-search/`). Отключить: `HDS_NO_AUTOSTART=1`.
7. **Диагностика** — `hds check`.

Лаунчеры ищут корень и бинарник `hds` в порядке `bin/` → `~/.local/bin/` → `PATH` → dev-сборка
(`HDS_ROOT` переопределяет корень). Логи веб-UI/автозапуска — `~/Library/Logs/hermes-disk-search/`.

> macOS-артефакт **не проверен на реальной машине**: чек-лист `tools/parity/MAC_CHECKLIST.md`.

## Архивы релиза

> Пошаговая инструкция «что скачать и в каком порядке запускать» — разделы выше:
> «Установка на Windows: по шагам» и «Установка — macOS». Ниже — технический состав ассетов:
> какой файл обязателен, а какой лишь запасной вариант.

| Ассет                                                         | Нужен?                     | Содержимое                                                                                                                                                                                                                                                                              |
| ------------------------------------------------------------------ | ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `hds-<версия>-windows-x64.zip` (+ `.sha256.txt`)         | **да (Windows)**        | основной архив:`bin\{hds,hds_mcp,llm_host}.exe`, `installers\`, `runtime-manifests\`, `sidecar\`, `assets\`, `shortcuts\`, `hermes-skill\`, `cline-rules\`, скрипты установки, `config.example.yaml`, `README.md`, `NOTICE.md`, `sha256.txt` |
| `hds-<версия>-macos-arm64.zip` (+ `.sha256.txt`)         | **да (macOS)**          | то же для Apple Silicon —**best-effort**                                                                                                                                                                                                                                            |
| `hds-engine-runtime-windows-x64-cuda.zip`                        | офлайн-установка | рантайм движка (LLM + ASR), скопированный из апстрима (NVIDIA/CUDA; есть и Vulkan-вариант для AMD/Intel) по`runtime-manifests\engine-manifest.json`; установщик скачивает его сам                                                                                                   |
| `hds-engine-runtime-macos-arm64-metal.zip`                       | офлайн-установка | то же для macOS (Metal) —**best-effort**                                                                                                                                                                                                                                            |
| `hermes-disk-search-<версия>-windows.zip` / `-macos.zip` | нет                          | архивы исходников (`git archive`), только для сборки из кода                                                                                                                                                                                               |
| `hds-client-<версия>-windows-x64.zip` (+ `.sha256.txt`) | для тонкого клиента | клиент для ноутбука (Cline без Hermes, M9): `bin\hds.exe`, мастер `setup-client.*`, правило/скилл, `version.txt`; тот же файл вложен в серверную поставку (`client/`) и отдаётся сервером по `/mcp/client-package.zip` |

Модели CLIP в релиз не входят (≈850 МБ) — отдельный тег `clip-onnx-v1`, установщик скачивает по
`runtime-manifests\clip-manifest.json`. GGUF/whisper — по URL из `installers\fetch_llm_models.ps1` /
`fetch_whisper_model.ps1`. Атрибуция и лицензии — `NOTICE.md`.

## Использование (CLI `hds`)

```powershell
# Первичная индексация (запустить на ночь; прогресс в консоли)
hds index
hds index --roots "D:\;E:\" --kinds text,pdf --progress-sec 3

# Поиск и RAG
hds search "техническое задание" --limit 8 [--kinds text,pdf] [--json]
hds ask "какие требования к срокам?" [--limit 8] [--json]

# Состояние и диагностика
hds status [--json]
hds check  [--json]      # компоненты окружения + роли (то же, что /api/diagnostics в UI)

# Инкремент/обслуживание
hds reindex <путь> [--no-force]
hds reindex-fts [--progress-sec N]     # перестроить лемматизированный FTS
hds forget <путь>
hds db-move --to <новый путь index.db> [--force]
hds clip-index                          # дозаполнить CLIP-векторы картинок
hds whisper-check [--file <медиа>] [--json]

# Процессы/сервисы
hds stop                                # создать index.stop (остановить индексацию)
hds watch [--roots a;b]                 # наблюдатель ФС
hds transcribe-watch                    # демон автотранскрибации (inbox_dir → out_dir)
hds transcribe-once <медиафайл>         # один файл «здесь и сейчас» (без очереди)
hds transcribe-stop                     # мягкая остановка демона (файл transcribe.stop)
hds ui [--host H] [--port N]            # веб-интерфейс (default 127.0.0.1:8765)
hds mcp [--http --host H --port N --path /mcp]   # MCP stdio или streamable-http
hds mcp-http check|start|stop|status|restart|run
```

Общее: `-h`/`--help`. Конфиг — `config.yaml` (путь можно переопределить `HDS_CONFIG`).
Индексация кооперативно останавливается файлом `index.stop` и приостанавливается `index.pause`.

БД живёт в WAL-режиме (`journal_mode=wal`, ключ `db.journal_mode`): **бэкап — только
через `VACUUM INTO`/`.backup`** (sqlite3), копировать файлы `index.db`/`index.db-wal`
«на горячую» нельзя (см. Э10). Ожидание блокировки — `db.busy_timeout_ms` (server —
5000, desktop — 30000). При `db.read_only_conns: true` (server-дефолт) читатели
(MCP/UI/CLI-поиск/`ask`) открывают базу read-only, писатель — только конвейер
индексации/watcher; `hds check` показывает фактические `journal_mode`/`busy_timeout`.

## Справочник команд: сервер, клиент, утилиты, установщики

> Плотная шпаргалка: всё, чем пользуются на **сервере** и на **клиенте** (ноутбук).
> Запускать `hds`/`hds.exe` — из-под того бинарника, который соответствует профилю машины
> (см. таблицу ниже). Развёрнутые пояснения — в разделах ниже, а серверная эксплуатация —
> `README_SERVER_MULTIUSER.md` §8d–§8e. Живой пример развёрнутого сервера с точными
> путями/значениями — `tools/parity/out/SERVER_MIGRATION_20261013.md` (артефакт, в git не идёт).

### 0. Где бинарник, конфиг и состояние

| Профиль                                                            | `hds.exe`                                                         | `config.yaml`                                       | состояние:`index.db`, `data/` (access.yaml, логи), PID, heartbeat |
| ------------------------------------------------------------------------- | ------------------------------------------------------------------- | ----------------------------------------------------- | ---------------------------------------------------------------------------------- |
| **Сервер** (`profile: server`)                              | `<install>\app\current\bin\hds.exe`                               | `%ProgramData%\hermes-disk-search\config.yaml`      | `%ProgramData%\hermes-disk-search\`                                              |
| **Desktop** (репо или распакованный релиз) | `target\{debug,release}\hds.exe` или `<install>\bin\hds.exe` | корень проекта/установки        | корень проекта (`data\`, `index.db`)                              |
| **Тонкий клиент** (ноутбук)                      | `<client>\bin\hds.exe`                                            | не нужен — всё считает сервер | —                                                                                 |

Резолвинг конфига (порядок): `HDS_CONFIG` (файл) → `HDS_ROOT` (каталог состояния) →
`<корень проекта>\config.yaml` → системный каталог состояния (если есть `profile: server`).
Скрипты репозитория находят бинарник сами через `hds_bin.ps1` (`bin\` → `target\release` → `target\debug`).

### 1. Службы (Windows SCM; только профиль `server`)

```powershell
# состояние и управление (роль: watch | mcp-http | ui | llm-host)
hds service status                  # все 4 роли (kind/state/autostart/detail)
hds service status <role>
hds service start   <role>          # sc.exe start hermes-disk-search-<role>
hds service stop    <role>
hds service install <role>          # auto start + авторестарт при падении (нужен админ)
hds service uninstall <role>        # sc.exe delete
hds service run     <role>          # то, что запускает SCM; вручную — блокирующий режим
```

* Имена служб: `hermes-disk-search-watch`, `-mcp-http`, `-ui`, `-llm-host` (LocalSystem, `START_TYPE auto`).
* Прямые команды SCM, когда `hds` недоступен: `sc.exe query|start|stop|delete hermes-disk-search-<role>`.
* Из веб-UI (admin Bearer): `GET /api/services`, `POST /api/services {role, action}`,
  `action = install|uninstall|start|stop|restart`.
* Порядок старта — `llm-host` → `watch` → `mcp-http` → `ui`; остановки — в обратном (`update.ps1 -Profile server` делает это сам).
* **Осторожно:** у службы настроены failure-actions (`restart/5000`), поэтому `sc stop`
  может не «удержаться» — служба поднимется через ~7 с. Чтобы остановить насовсем:
  `sc failure hermes-disk-search-watch reset= 0 actions= ""` или `sc config … start= demand`.

### 2. Индексация и обслуживание индекса

```powershell
hds index                            # инкрементный обход index.roots (это же делает watch)
hds index --full                     # перечитать всё (по mtime/размеру не пропускать)
hds index --roots "D:\;E:\Archive"   # разовые корни (через ';')
hds index --kinds text,pdf           # ограничить виды
hds index --limit 20 --no-prune      # пробный прогон: не больше 20 файлов, без prune
hds index --rechunk                  # перечанковать (после смены chunk.size)
hds index --progress-sec 10 --quiet  # частота прогресса / тишина
hds watch [--roots "D:\"]            # резидентный обход + FS-события (роль службы watch)
hds reindex <path> [--no-force]      # один файл/папка
hds reindex-fts [--progress-sec N]   # перестроить лемматизированный FTS-индекс
hds forget <path>                    # удалить файл/папку из индекса
hds clip-index                       # догнать CLIP-векторы картинок
hds status [--json]                  # файлы по видам/статусам, чанки, ошибки
hds db-move --to <path> [--force]    # перенести БД (останавливает индексацию, переписывает db_path)
hds stop                             # сигнал остановки индексации: файл index.stop
```

Файлы-сигналы в **корне проекта** (для службы это `<install>\app\current\`):
`index.pause` — пауза обхода (`hds index` её ждёт; ставит в т.ч. резидент при заданиях
`gpu.pause_index_on_query`), `index.stop` — остановка, `index.heartbeat.json` — прогресс,
`watch.lock` — единственный watcher на машину. `hds watch` файлы `index.pause`/`index.stop`
**не читает** — обход watcher'а останавливается только остановкой службы.

### 3. Поиск и RAG-ответ

```powershell
hds search "договор аренды"                 # гибрид (FTS + векторы + CLIP) — мгновенно
hds search "SAP" --kinds docx,pdf --limit 20
hds search "отчёт" --json                   # машинный вывод (из пути/страницы)
hds ask "о чём договор с подрядчиком" --limit 15
hds ask "какие есть ТЗ" --limit 30 --json   # обзорный RAG-ответ с цитатами [N]
```

`ask` идёт через локальную chat-роль: 15–20 ток/с, минуты на ответ — это норма.
Те же операции по MCP: `search_local_files`, `ask_my_files`, `index_status`,
`start_indexing`, `stop_indexing`, `reindex_path`.

### 4. Доступ: пользователи и токены (`data/access.yaml`)

```powershell
# пользователи (проекты = «корень → проект» из index.roots)
hds user add    --login laptop --password "<пароль>" [--name "Ноутбук"] [--projects "D:\;E:\Archive"] [--admin]
hds user list
hds user passwd --login laptop --password "<новый пароль>"   # отзывает все токены этого логина
hds user del    --login laptop
hds user grant  --login laptop --projects "D:\"
hds user revoke --login laptop --projects "D:\"

# токены (plain-токен показывается ОДИН раз; в БД/логи идёт только sha256)
hds token new    --user laptop [--ttl-days 90]     # → строка токена на stdout
hds token list
hds token revoke --user laptop
hds token revoke --sha <префикс хэша>              # префикс из token list

# токен без помощи админа (логин/пароль → токен), эндпоинт /auth/login на UI-порту
hds login --login laptop --password "<пароль>" --url http://192.168.0.101:8765
```

* Пароли — Argon2id (хэш); токен — opaque 256 бит, в файле только sha256, TTL из `access.token_ttl_days`.
* `--admin` видит все проекты и может управлять индексацией/службами; обычный пользователь
  видит только выданные проекты (фильтрация выдачи MCP/UI).
* Гейт включается **только** `access.enabled: true` — сам `profile: server` доступа не даёт.
* Аудит (server-профиль — включён по умолчанию): `%ProgramData%\hermes-disk-search\data\logs\audit.jsonl`
  (`login_ok`/`login_fail`/`unauthorized`/`tool_call`/`admin_op`/`token_revoke`).

### 5. MCP-сервер, веб-UI, синхронизация Cline

```powershell
hds mcp                                   # stdio: клиент поднимает процесс сам (локальный режим)
hds mcp --http --host 0.0.0.0 --port 8787 --path /mcp
hds mcp-http check | start | stop | status | restart | run [--host H --port N --path /mcp]
hds ui [--host H] [--port N]              # дефолт 127.0.0.1:8765; в server-профиле — headless
hds cline-sync [--json] [--dry-run]       # локальный Cline из config.yaml (порт/путь берутся из конфига)
hds cline-sync --server-url http://192.168.0.101:8787 --token <T>
```

* `--server-url` указывайте **без** `/mcp` — путь добавляется сам (с `…/mcp` получится `/mcp/mcp`).
  Токен можно передать переменной `HDS_CLINE_TOKEN` вместо `--token`.
* Эндпоинты: `/health` — публичный (MCP: `200 {"app":"disk-search",…}`; UI в server-профиле: `401`);
  `/mcp` — JSON-RPC + `Authorization: Bearer`; UI — `/`, `/auth/login`, `/api/*`
  (`status`, `diagnostics`, `services`, `access/users|projects|token/…`, `config/save`,
  `index/reindex`, `db/move`, …). В **server-режиме** (`access.enabled: true`) вся UI — и страница,
  и `/api/*` — доступна только с **админ-токеном**: `http://<host>:8765/?token=<токен>` (без него —
  `401`); в десктопе токен не нужен.
* Файлы Cline: `%USERPROFILE%\.cline\data\settings\cline_mcp_settings.json`, `%USERPROFILE%\.cline\mcp.json`
  (+ `models.json`, `rules\disk-search.md`, `skills\disk-search\SKILL.md`); после правок — перезапуск Cline.
* Ключи конфига: `mcp_http.{host,port,path,autostart,start_timeout}`, `ui.{host,port}`,
  `access.{enabled,token_ttl_days}`, `rate_limit.*`, `audit.*`, `trusted_proxy`, `http.max_body_kb`.
* Публикация в LAN: `mcp_http.host`/`ui.host = 0.0.0.0` + правило брандмауэра (§8).
  Наружу — только через TLS-reverse-proxy (`installers\proxy\{Caddyfile,nginx.conf}`) и `trusted_proxy: true`.

### 6. Резидент `llm-host` (владелец GPU, порты 8010–8012) и диагностика

```powershell
llm_host.exe status                    # инстансы/порты/VRAM
llm_host.exe devices                   # устройства движка (нумерация для gpu.device_index)
llm_host.exe load   <role>             # role: chat | embedding | rerank | whisper | transcribe
llm_host.exe unload <role>
llm_host.exe run                       # резидентный режим (то, что запускает служба llm-host)
llm_host.exe stop [--force]            # мягкая остановка; --force — по pid-файлу, если HTTP завис
# общие флаги: --config FILE --runtime DIR --engine-dir DIR --host HOST --port N
```

```powershell
hds check [--json]                     # профиль, БД, корни, chat/embedding/rerank, OCR, ffmpeg,
                                       # лемматизатор (pymorphy3), .mpp, GPU/VRAM, аудит, очередь, периметр
hds whisper-check [--file <медиа>] [--json]
hds transcribe-watch                   # демон автотранскрибации (inbox_dir → out_dir)
hds transcribe-once <медиафайл>        # один файл «здесь и сейчас», без очереди
hds transcribe-stop                    # мягкая остановка демона (сигнал transcribe.stop)
```

* Фасад совместим с llama-server: `:8010` chat, `:8011` embedding, `:8012` rerank — слушают только `127.0.0.1`.
* Логи/состояние: `<state>\data\logs\llm-host.log`, `<state>\data\llm-host.heartbeat.json`, `<state>\data\llm-host.pid`.
* Быстрая приёмка сервера: `sc query hermes-disk-search-<role>` ↔ `GET /api/services` (admin Bearer),
  `GET /health` на 8787 и 8765, `hds check`.

### 7. Скрипты-утилиты (релиз/репозиторий)

| Скрипт                                                         | Что делает                                                                                                                                                    | Повторный запуск                                      |
| -------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| `hds_bin.ps1`                                                      | резолвер бинарника (`bin\` → `target\release` → `target\debug`); подключается как `. .\hds_bin.ps1`                          | безвреден                                                   |
| `run_index.ps1`                                                    | один инкрементный обход (`hds index`)                                                                                                           | безвреден (пропустит неизменившееся) |
| `run_ui.ps1`                                                       | поднять UI и открыть браузер; если порт уже отвечает — просто открывает; в server-профиле — headless | безвреден                                                   |
| `hidden_launch.vbs`                                                | запуск без окна консоли (задачи/Startup)                                                                                                     | безвреден                                                   |
| `installers\cline_mcp_merge.py`                                    | слияние MCP-записей в настройках Cline (используется`cline-sync`)                                                               | идемпотентно                                             |
| `installers\configure_ocr.py`                                      | Tesseract/tessdata: путь, языковые пакеты,`TESSDATA_PREFIX`                                                                                        | идемпотентно                                             |
| `installers\publish_engine_patch.ps1`, `publish_clip_models.ps1` | публикация патча движка / CLIP-моделей как релизных ассетов                                                              | идемпотентно                                             |

### 8. Установка и обслуживание сервера (все шаги идемпотентны)

```powershell
# 1) релиз: dist\hds-<ver>-windows-x64\ + .zip + .zip.sha256.txt
powershell -File installers\build_rust_release.ps1 -Version 1.0.1 [-SkipBuild] [-SkipZip] [-WithSidecar -SidecarDir <dir>]
# 1a) самодостаточный sidecar (портативный CPython + копия пакета hds; нужен uv, ~250 МБ загрузки)
powershell -File installers\build_sidecar.ps1 -OutDir sidecar [-SelfTest] [-Force]

# 2) раскладка: app\<ver>\ + junction app\current
powershell -File installers\install_app_version.ps1 -From dist\hds-1.0.1-windows-x64 -Version 1.0.1 -Root C:\Users\Sasha\hds-server -SetCurrent [-Force]

# 3) конфиг положить в каталог состояния ДО установщика (он существующий не перезапишет):
#    %ProgramData%\hermes-disk-search\config.yaml  (profile: server, access.enabled: true, mcp_http/ui.host)

# 4) установка служб (админ; -WhatIf — план без изменений и без прав администратора)
powershell -File <install>\app\current\installers\install_server.ps1 -WhatIf -StateDir "%ProgramData%\hermes-disk-search" -McpHost 0.0.0.0 -McpPort 8787 -UiHost 0.0.0.0 -UiPort 8765 -Parallel 1 -GpuDevice 1 -Roots "D:\" -EnableAccess -AdminLogin admin -AdminPassword "<secret>" -NoFetch [-KeepAtLogOn] [-Roles "watch,mcp-http,ui,llm-host"]

# 5) старт (порядок: резидент → обход → MCP → UI)
hds service start llm-host; hds service start watch; hds service start mcp-http; hds service start ui

# 6) приёмка
hds check
hds token new --user admin                              # выдать токен (показывается один раз)
hds cline-sync --server-url http://127.0.0.1:8787 --token <T>   # локальному Cline
```

Повторный запуск установщиков:

* `install_server.ps1` — существующий `config.yaml` **не** перезаписывает; службы создаёт/обновляет
  заново; `-Roles ""` = только конфиг (права админа не нужны); `-WhatIf` ничего не меняет;
  `-KeepAtLogOn` не трогает десктопные задачи `HermesDiskSearch*` (без него релизная версия
  пытается их удалить и падает, если задач нет).
* `install_app_version.ps1` — `data\`/`models\` общие (junctions), `-Force` переустанавливает версию.
* `update.ps1` — останов → подмена версии → старт (`-Profile server` для служб, `desktop` — задачи
  Планировщика); `-ListVersions`, `-RollbackTo <ver>`, `-WhatIf`.
* `fetch_*.ps1` — идемпотентны: проверяют sha256/размер и без `-Force` ничего не качают.
  `fetch_engine_runtime.ps1 [-Backend auto|cuda|vulkan|metal|cpu] [-PatchEngine|-RollbackEnginePatch]`;
  `fetch_llm_models.ps1 [-Models chat,embedding,rerank] [-RuntimeDir DIR] [-Force]`;
  `fetch_whisper_model.ps1` / `fetch_diarization_model.ps1` / `fetch_clip_models.ps1 [-Force]`.

```powershell
# обновление сервера и откат
powershell -File <install>\app\current\installers\update.ps1 -Root <install> -Profile server -From dist\hds-1.1.0-windows-x64.zip -Version 1.1.0
powershell -File <install>\app\current\installers\update.ps1 -Root <install> -ListVersions
powershell -File <install>\app\current\installers\update.ps1 -Root <install> -Profile server -RollbackTo 1.0.1
```

```powershell
# то, что установщик/миграция делает руками
New-NetFirewallRule -DisplayName 'hermes-disk-search TCP 8787' -Direction Inbound -Action Allow -Protocol TCP -LocalPort 8787 -RemoteAddress LocalSubnet
Remove-NetFirewallRule -DisplayName 'hermes-disk-search TCP 8787'
icacls "%ProgramData%\hermes-disk-search" /grant "<ДОМЕН\Пользователь>:(OI)(CI)(M)" /T   # иначе `hds token new` → os error 5
cmd /c mklink /J "%windir%\System32\config\systemprofile\AppData\Local\llama-runtime" "%LOCALAPPDATA%\llama-runtime"
cmd /c mklink /J "%windir%\System32\config\systemprofile\AppData\Roaming\OpenResearchTools" "%APPDATA%\OpenResearchTools"
```

Прочие установщики: `setup.cmd` → `setup.ps1 [-SkipModels -SkipEngine -NoAutostart -SkipIntegrations -SmokeTest]`
(desktop); `install_autostart.ps1 [-StartupFolder]`; `installers\install_llm_host_task.ps1 [-Exe -Config -Start -Status -Remove -Force -StartupFolder]`; `installers\install_ui_task.ps1 [-Port -Status -Remove]`; `install_hermes.ps1 [-HermesDir]`; `install_cline.ps1`;
macOS: `installers\install_macos.command`, `installers\install_server.command`, `installers\install_client.command`.

### 9. Клиент (ноутбук): подключение к серверу

```powershell
# 1) НА СЕРВЕРЕ — учётка и токен для ноутбука
hds user add --login laptop --password "<пароль>" [--projects "D:\"]
hds token new --user laptop --ttl-days 90          # plain-токен показывается один раз → отдать на ноутбук

# 2) НА НОУТБУКЕ — получить клиентский пакет (права админа не нужны), два канала:
#    а) со страницы релизов: hds-client-<версия>-windows-x64.zip (+ .sha256.txt)
#    б) с сервера, своим токеном (тот же пакет той же версии, что у сервера):
curl.exe -H "Authorization: Bearer <ТОКЕН>" -o hds-client.zip http://192.168.0.101:8787/mcp/client-package.zip
Expand-Archive .\hds-client.zip -DestinationPath .\hds-client -Force
Get-ChildItem .\hds-client -Recurse | Unblock-File          # сняли «скачано из интернета»
#    (dev-вариант: собрать пакет самому из репозитория —
#     powershell -File installers\build_client_release.ps1 -Version <версия>)

# 3) НА НОУТБУКЕ — подключить Cline мастером
cd .\hds-client
.\setup-client.ps1 -ServerUrl http://192.168.0.101:8787 -Token <ТОКЕН> [-McpPath /mcp] [-DryRun] [-SkipHealth]
#   проще двойным щелчком:  setup-client.cmd -ServerUrl http://192.168.0.101:8787 -Token <ТОКЕН>
# macOS (пакет hds-client-<версия>-macos-arm64.zip; на скачанный — xattr -dr com.apple.quarantine):
bash install_client.command --server-url http://192.168.0.101:8787 --token <ТОКЕН> [--dry-run] [--skip-health]

# 4) вручную (без мастера)
hds cline-sync --server-url http://192.168.0.101:8787 --token <ТОКЕН>
hds login --login laptop --password "<пароль>" --url http://192.168.0.101:8765   # токен самому (эндпоинт /auth/login)

# 5) проверка соединения с ноутбука
Invoke-WebRequest 'http://192.168.0.101:8787/health' -UseBasicParsing        # 200 {"app":"disk-search","version":…}
```

* **Откуда пакет:** (а) ассет релиза `hds-client-<версия>-<платформа>.zip` — тот же файл,
  что вложен в серверную поставку; (б) скачать с сервера по
  `<ServerUrl>/mcp/client-package.zip` с заголовком `Authorization: Bearer <ТОКЕН>`
  (гейт как у `/mcp`; так удобнее, когда GitHub недоступен). Через reverse-proxy путь
  работает: он внутри опубликованного `/mcp`-префикса.
* **Версии:** пакет несёт `version.txt`, сервер отдаёт свою версию в `/health`. Мастер
  сверяет их и при расхождении предупреждает — правило/скилл и схема инструментов могли
  отстать (W7).
* Мастер: проверяет `GET <ServerUrl>/health`, пишет MCP-запись (`type: streamableHttp`,
  заголовок `Authorization: Bearer …`) в оба файла Cline, кладёт правило и скилл, затем просит
  перезапустить Cline. Повторный запуск ничего не дублирует (`-DryRun` — только план).
* `-ServerUrl` указывайте **без** `/mcp` (для LAN: `http://<IP-сервера>:8787`; при TLS-прокси — `https://host/`).
* В клиентском пакете нет движка/моделей/индекса/sidecar — всё считает сервер.

### 10. Диагностика и частые грабли

| Симптом                                                                             | Что проверить                                                                                                                                                                                                                                                                                   |
| ------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| MCP-инструменты не отвечают                                           | `GET http://<host>:8787/health`; `hds service status mcp-http`; `sc query hermes-disk-search-mcp-http`; токен в настройках Cline (401 = нет/просрочен)                                                                                                                    |
| UI отдаёт 401                                                                        | это норма в server-профиле (доступ по токену/логину):`POST /auth/login`, админ — `?token=…`                                                                                                                                                                  |
| Индексация «стоит»                                                        | `index.heartbeat.json` → `"paused": true` = в корне установки лежит `index.pause` (его ставит резидент под задания и может не снять); удалить файл. Признак: heartbeat обновляется, а CPU службы = 0 |
| `hds token new` → `os error 5`                                                        | ACL каталога состояния (§8,`icacls`)                                                                                                                                                                                                                                                    |
| Служба поднимается после`sc stop`                                  | failure-actions (`sc failure … reset= 0 actions= ""`, §1)                                                                                                                                                                                                                                               |
| Служба не находит модели/движок                                 | junctions`systemprofile` + machine-env `LLAMA_RUNTIME_DIR`/`HDS_ENGINE_DIR`/`HDS_EXTRACT_PYTHON` (§8)                                                                                                                                                                                              |
| Извлечение падает («воркер: поток вывода закрыт») | в корне установки нужны`config.yaml` и пакет `hds` (junction/symlink или сборка `-WithSidecar`, §8)                                                                                                                                                               |
| Куда смотреть логи                                                         | `<state>\data\logs\{audit.jsonl,llm-host.log}`, `GET /api/diagnostics`; брандмауэр — `Get-NetFirewallRule -DisplayName 'hermes-disk-search*'`                                                                                                                                              |

Полезные ссылки: `README_SERVER_MULTIUSER.md` (мультиюзер-сервер, §8d–§8e — установка и клиент),
`installers\client_readme.md` (клиентский пакет), `tools\parity\out\SERVER_MIGRATION_20261013.md`
(живой пример развёртывания + найденные дефекты), `installers\proxy\` (образцы TLS-прокси).

## Веб-интерфейс (`hds ui`)

> ⚠️ **Доступ к UI (server-режим).** В десктопе (`access.enabled` не задан) UI открывается без
> токена. При **`access.enabled: true`** страница **и все `/api/*`** требуют **админ-токен**: открывайте
> `http://<host>:<port>/?token=<админ-токен>` — токен запоминается в браузере и подставляется во все
> запросы; без него страница и API отдают `401` (в server-профиле и `/health` отвечает `401`).
> Токен админа выписывают `hds login --login admin --password …` / `hds token new --user admin`
> (plain — один раз) или кнопка «Выдать токен» в карточке «Доступ»; пароль/токены — в
> `data/access.yaml` каталога состояния. Подробности — `README_SERVER_MULTIUSER.md` §6.

Ярлык «Hermes Disk Search» (рабочий стол) запускает `run_ui.ps1` → `bin\hds.exe ui` и открывает
`http://127.0.0.1:8765`. Это интерфейс **администрирования** (не запросов): статус индекса и ролей
`llm-host`, дерево индексации по БД (`/api/tree`, кэш 30 с, `?refresh=1`), диагностика
(`/api/diagnostics` = полный `hds check`), управление индексацией (`start`/`stop`/`pause`/`resume`),
перезапуск резидента (`/api/llm-host/restart`), смена модели чата из общего рантайма
(`/api/chat-model`, `POST /api/chat-model/set` — с перезапуском `llm-host`), запуск/остановка
общего MCP-сервера (`/api/mcp-http`), правка `config.yaml` (roots/exclude_paths) **с сохранением
комментариев**, синхронизация настроек Cline, транскрибация. Перезапуск `llm-host` и смена модели
идут **фоновым заданием**: карточка показывает этапы и таймер (`GET /api/llm-host/job`), не выглядя
«зависшей» во время загрузки модели. Поиск/RAG — через агентов (Cline), MCP-инструменты и CLI
(`hds search`/`hds ask`). Страница шлёт `X-HDS-UI: 1` (CSRF-защита POST).

На **медленных машинах** (LLM на интегрированной графике, ходы Cline не успевают за таймаутом
клиента): включите `cline.slow_machine: true` в `config.yaml` и выполните `hds cline-sync` —
агенту установится правило экономного поиска (один вызов инструмента за ход, `limit` ≤ 5,
без web_search). Брошенный запрос, который клиент оборвал по таймауту, снимает **страж
резидента** (`gpu.abandoned_request_sec`, по умолчанию 900 с): GPU не «дожёвывает» его
минутами — резидент штатно завершается и поднимается заново.

Логи сервера: `%LOCALAPPDATA%\hermes-disk-search\ui.log` и `ui.err.log`.
На странице две закладки: **«Поиск»** (всё перечисленное выше) и **«Транскрибация»**
(см. следующий раздел).

На закладке «Поиск» есть блок **«Демон индексации»**: состояние процесса `hds watch`
(по файлу `watch.lock`, плюс признаки `index.pause`/`index.stop` и «индексация идёт»)
и кнопки **Запустить / Остановить / Перезапустить**. Остановка мягкая (файл
`index.stop`), запуск — без окна. `hds watch` — **отдельный** процесс: с `llm-host`
он не связан (тот перезапускается своей кнопкой в блоке «Резидент llm-host»).

## Автотранскрибация (`hds transcribe-watch`)

Отдельный демон (`PLAN_AUTO_TRANSCRIBE`, вариант A): видит **готовый** медиафайл в
`inbox_dir`, прогоняет транскрибацию **и** диаризацию одним вызовом движка
(`mode: transcript`; владелец GPU — `llm-host`) и пишет `.md` с метками спикеров в
`out_dir`, после чего **удаляет исходник** — только при успехе.

```yaml
auto_transcribe:
  enabled: true
  inbox_dir: 'D:\media_in'    # сюда кладём медиа
  out_dir:   'D:\media_out'   # здесь появляются транскрипты (индексируются)
```

* **Команды:** `hds transcribe-watch` (демон; автозапуск — задача
  `HermesDiskSearchTranscribe`), `hds transcribe-once <файл>` (один файл «здесь и
  сейчас»), `hds transcribe-stop` (мягкая остановка через файл `transcribe.stop`).
  Дубль демона не поднимется — его держит `transcribe.lock`.
* **Формат выхода — как отдаёт движок:** `### SPEAKER_00 [00:00:01 - 00:00:11]`,
  неприсвоенная реплика — `### UNASSIGNED […]`. Рядом лежат `<stem>.orig.md`
  (текст до подстановки имён) и `<stem>.speakers.json` (карта «метка → имя»).
* **Диаризация обязательна:** без sortformer-модели задание падает
  (`installers\fetch_diarization_model.ps1`, ≈449 МБ) — деградации в `speech` нет.
* **Приоритеты (ТЗ):** 1) поиск/чат → 2) автотранскрибация → 3) индексация
  (`gpu.priorities.transcribe = 60`): задание ставит `index.pause` и вытесняет
  индексные роли; запрос чата вытеснить транскрибацию может, индексация — нет.
* **Утилизация исходника:** `source_disposal: delete` (по умолчанию) | `keep` | `move`
  (+`source_disposal_dir`).
* **UI:** закладка «Транскрибация» — список файлов `out_dir` с превью, кнопка
  «Присвоить имена спикерам» (таблица Спикер|Имя, подставляются **только заполненные**
  строки; перед правкой создаётся `<файл>.bak`), «Перезапустить задание» и поля
  `inbox_dir`/`out_dir` с сохранением в `config.yaml`.
* **Демоном можно управлять из UI** (блок «Демон автотранскрибации»): состояние
  (работает / остановлен / `lock` без процесса), кнопки **Запустить**, **Остановить**,
  **Перезапустить**. Остановка мягкая (`transcribe.stop`), запуск — без окна, тем же
  способом, что резидент в блоке «Резидент llm-host»; дубль невозможен (`transcribe.lock`).
  Если `enabled: false`, запуск честно предупредит, что демон завершится сам.
* **Идемпотентность:** манифест `data/auto-transcribe/manifest.jsonl` и журнал очереди
  `data/auto-transcribe-queue.jsonl` — уже обработанный файл и свежий выход повторно не
  берутся (`overwrite_existing: false`), число попыток ограничено `max_attempts`.

## MCP-сервер

* **stdio** — `bin\hds_mcp.exe` (он же `hds mcp`): каждый клиент поднимает свой процесс.
  Инструменты (6): `search_local_files`, `ask_my_files`, `index_status`, `start_indexing`,
  `stop_indexing`, `reindex_path`.
* **streamable-http** — ОДИН инстанс на машину: `hds mcp --http` (URL по умолчанию
  `http://127.0.0.1:8787/mcp`); управление — `hds mcp-http check|start|stop|status|restart|run`
  (`restart` переиспользует живой инстанс, но перезапускает устаревший).

Интеграции подключают клиентов **по URL** — они не запускают свои процессы (иначе плодятся сироты).

## Интеграция с Hermes / Cline

**Windows:**

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File install_hermes.ps1   # Hermes Desktop
powershell -NoProfile -ExecutionPolicy Bypass -File install_cline.ps1    # Cline Desktop/CLI
```

**macOS:**

```bash
bash installers/install_hermes_macos.sh    # Hermes Desktop
bash installers/install_cline_macos.sh     # Cline Desktop/CLI
```

Идемпотентно: регистрируют MCP-сервер disk-search (URL общего инстанса `:8787`, определённый из
`config.yaml`; для Hermes — с фолбэком на stdio `bin\hds_mcp.exe`), поднимают сервер
(`hds mcp-http restart`) и кладут скилл (`hermes-skill/`). При необходимости правят `.env` Hermes —
блок `NO_PROXY=localhost,127.0.0.1,::1` (иначе httpx2/движок шлёт локальные запросы в системный
прокси, и MCP отвечает 503). Настройки Cline обновляет общий хелпер `installers/cline_mcp_merge.py`
(на macOS берётся python воркера `sidecar/`). Можно запускать до установки клиента и повторить позже.

### Что доходит до модели и как настройки Cline синхронизируются

Сервер отдаёт «системный промпт» в ответе на `initialize` (поле `instructions` — спека MCP), но
клиенты вправе его игнорировать: **Cline 0.0.43 его не подмешивает**. Поэтому для Cline та же суть
продублирована двумя артефактами, и оба ставит одна команда:

| Что                                       | Куда ставится                | Когда попадает в промпт                                                                              |
| -------------------------------------------- | ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| Правило`cline-rules/disk-search.md` | `~/.cline/rules/disk-search.md`        | **всегда**: правила инжектятся в системный промпт каждой сессии |
| Скилл`hermes-skill/disk-search.md`    | `~/.cline/skills/disk-search/SKILL.md` | лениво: только если модель вызовет`use_skill`                                             |

Локальная 9B-модель скилл сама не активирует и отвечает по одному запросу — «найди ТЗ» отвечает
первой выдачей из 8 фрагментов. Правило доносит до модели главное: одна выдача — это фрагменты, а
не список файлов, и на обзорные вопросы нужно несколько запросов разными формулировками.

**Синхронизация настроек Cline** (`hds cline-sync`, кнопка «Синхронизировать настройки Cline» в
веб-интерфейсе, а также `install_cline.ps1` / `installers/install_cline_macos.sh` при установке
релиза — один и тот же код `hds_core::cline::sync`):

1. **окна контекста моделей** (`~/.cline/data/settings/models.json`): `contextWindow` и
   `maxInputTokens` моделей нашего провайдера (`baseUrl` = `chat.base_url`) приравниваются
   реальному слоту `llm-host` (`llm_server.<role>.ctx_per_slot`, при необходимости —
   `llm.<role>.n_ctx`); алиасы разводятся по ролям (`chat`/`chat-think` → чат, `embedding*`,
   `rerank*`). Без этого Cline сжимает историю раньше заполнения слота и теряет контекст поиска;
2. **MCP-сервер** disk-search в `data/settings/cline_mcp_settings.json` и в `mcp.json`
   (URL из `mcp_http.*`, `autoApprove` на 6 инструментов, `timeout: 300`), чужие серверы
   сохраняются;
3. **правило и скилл** (см. таблицу выше).

Отчёт (в UI — списком, в CLI — строками или `--json`, есть `--dry-run`) говорит, что изменилось, и
**предупреждает о перезапуске Cline**: `models.json` и настройки MCP читаются при его старте, правило
и скилл — при старте новой сессии.

Потолок вывода роли `chat` задаётся `chat.max_tokens` (см. «Конфиг»): у агентов он был 600 токенов
вместе с размышлениями, и ответы обрывались на полуслове. Ответ `ask_my_files` — отдельный,
короткий бюджет `chat.rag_max_tokens`.

Агенты подключаются **по URL** — своих процессов не запускают (иначе плодятся сироты). Инструменты
MCP — 6 (см. «MCP-сервер»). На macOS лаунчеры веб-UI и индексации ставит `install_macos.command`
на рабочий стол; автозапуск — LaunchAgent-ы `local.hds.*`.

## llm-host — резидент (владелец GPU и портов 8010–8012)

`bin\llm_host.exe run` — единый процесс: держит роли **chat** (`:8010`), **embedding** (`:8011`),
**rerank** (`:8012`) на движке openresearchtools и **сам ASR** (whisper через движок), раздаёт
OpenAI-совместимый HTTP и `/internal/*` (`status`, `transcribe`).

```powershell
bin\llm_host.exe run       # запустить резидент (владелец портов)
bin\llm_host.exe status    # отчёт: роли, VRAM, пауза
bin\llm_host.exe stop      # graceful: выгрузить инстансы, снять pid
```

* **VRAM-диспетчер** (`gpu.policy`): `query_priority` (запрос важнее — индексация на паузу),
  `indexing_priority`, `manual`. Вытесняет роли по приоритетам и **уважает** пользовательскую
  `index.pause` (не снимает её).
* Рантайм движка — `%APPDATA%\OpenResearchTools\TranscribeOffline\Engine` (переопределяется
  `HDS_ENGINE_DIR`/`index.whisper_engine_dir`); GGUF-модели — общий `%LOCALAPPDATA%\llama-runtime`.
* Автозапуск — задача Планировщика `HermesDiskSearchLlmHost` (`installers\install_llm_host_task.ps1`).

### Слоты чата и SLA (M4): сколько одновременных RAG выдерживает машина

`llm_server.parallel` — число **слотов** чата (дефолт профиля: `server` → 5, `desktop` → 1).
Общий контекст инстанса = `parallel × llm_server.chat.ctx_per_slot`, KV-кэш растёт линейно по
общим токенам. Перед стартом `llm-host` считает «модель + KV×parallel + compute-буфер» и сверяет
со свободной VRAM (минус `gpu.reserve_mb`): **не влезает — явная ошибка с числами** и подсказкой
снизить `parallel`/`ctx_per_slot` (молчаливой деградации нет).

Замеры на RTX 3060 12 ГБ (08.10.2026, `tools/parity/out/m4_slots_*.json`; 5 клиентов шлют
`/v1/chat/completions` одновременно, ответ 300 токенов, чат-модель Q6_K + KV `q8_0`):

| `parallel` | `ctx_per_slot`                                        | VRAM (наш процесс) | 5 запросов: wall                                                                                                       | p95       |
| ------------ | ------------------------------------------------------- | ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------ | --------- |
| 2            | 32768                                                   | 8129 МиБ                  | 33,7 с                                                                                                                        | ≈33 с   |
| 3            | 32768                                                   | 8723 МиБ                  | 27,1 с                                                                                                                        | ≈26,5 с |
| 5            | 16384                                                   | 8553 МиБ                  | **21,3 с** (все параллельно)                                                                              | ≈20,8 с |
| 5            | 32768                                                   | —                           | **отказ** гейта: нужно 10668 + резерв 1024, свободно 11385 (не хватает 307 МиБ) | —        |
| 5            | 65536 (боевые 65536 симв. контекста) | —                           | **отказ** гейта: нужно 13388 + 1024 > 11385                                                               | —        |

Вывод: при живом рабочем столе на карту влезают `parallel: 5` с `ctx_per_slot: 16384`
(тогда держите `chat.max_context_chars` ≲ 32000) либо `parallel: 2–3` с `ctx_per_slot: 32768`.
До `parallel`-фикса M4 слоты простаивали: вызовы шли строго по очереди (43,4 с на те же 5 запросов)
— мьютекс шлюза держался всю генерацию, а `n_parallel` вообще не доходил до инстанса движка.

### Очередь тяжёлых RAG и fair-share (M4 T3)

`ask_my_files` (тяжёлый RAG) в server-профиле при занятых слотах не отклоняется, а
**ждёт**: round-robin между пользователями (ключ — логин из Bearer, иначе IP) и FIFO
внутри пользователя — «жадный» пользователь не забирает все слоты, пока ждут другие.
Очередь полна → `429` (`queue.max_depth`); не дождался за `queue.wait_timeout_sec` →
`503`; оба с `Retry-After` (+ `X-Queue-Depth`). Обрыв клиента в ожидании освобождает
место.

```yaml
queue:
  enabled: true            # server по умолчанию; desktop — выключено (поведение M3)
  max_depth: 20            # сколько ЖДУТ (исполнение ограничивает rate_limit.max_inflight)
  wait_timeout_sec: 240    # держите < 300 — таймаута MCP-клиента
  policy: rr               # rr (fair-share) | fifo (строго по времени прихода)
```

Метрики очереди (глубина, ожидание p50/p95, отказы, «кто кого ждёт») видны в `hds check`
и `/api/diagnostics`: процесс `hds mcp-http` пишет снапшот
`<state_dir>/data/mcp-queue.json` раз в 2 с, а `check` предупреждает, если статус не
пишется или устарел (MCP не запущен). Ожидание пишется в аудит событием `queue_wait`.
Ограничение: очередь живёт в `hds mcp-http`; админский `ask` веб-UI лимитируется своими
лимитами M3 (общая очередь на два процесса — вне M4).

### Квоты и приоритеты по пользователю (M4 T4)

Ключ лимитов и квот — **пользователь**, а не токен: `user:<логин>` по валидному Bearer,
иначе `ip:<адрес>`. Поэтому «два токена одного пользователя» делят одну квоту
(исправлен дефект M3 F10.3); `rate_limit.per_token: true` возвращает прежний
пер-токенный ключ, но частота всё равно дополнительно ограничивается агрегатом на
пользователя (`requests_per_min_per_user`).

```yaml
rate_limit:
  max_inflight: 5            # ИСПОЛНЕНИЕ (глобально; лимит слотов очереди); дефолт по профилю:
                             #   server 5 (= llm_server.parallel), desktop 4. Держите >=
                             #   llm_server.parallel — hds check предупредит о рассинхроне
  max_inflight_per_user: 2   # ...и на пользователя: 3-й одновременный RAG того же
                             #   пользователя -> 429 + hint (rate_limit.max_inflight_per_user)
  requests_per_min_per_user: 120  # агрегат частоты на пользователя (при per_token: true)
  per_token: false           # true = ключ лимитов — токен (поведение M3)
```

Пер-пользовательский предел исполнения учитывается, только если он **строже**
глобального `max_inflight` (иначе он ничего не добавляет — запрос просто ждёт слот в
очереди). В очереди (T3) отказ по нему — немедленный `429` с `X-Queue-Depth: 0` и
hint'ом; счётчик виден в снапшоте (`rejected_user`) и в `hds check`. Без очереди
(desktop / `queue.enabled: false`) тот же предел применяет лимитер (эффективный
предел = `min(max_inflight_per_user, max_inflight)`).

Инвариант «индексация уступает» обеспечивают M4 T1 (WAL + `db.read_only_conns`:
читатели RAG не ждут писателя БД) и `gpu.pause_index_on_query` (файл `index.pause`,
владелец — `hds-llama`): тяжёлый RAG не блокируется индексным писателем, а индекс
встаёт на паузу на время запроса.

### Нагрузочная приёмка 5 × `ask_my_files` (M4 T5)

Стенд `tools/parity/load5.py` (без сторонних зависимостей) поднимает песочный
`llm_host` с заданными `parallel`/`ctx_per_slot` и песочный `hds mcp-http`
(server-профиль, очередь включена, 5 токенов/пользователей), после чего шлёт
**5 одновременных реальных `ask_my_files`** через `/mcp`; отчёты —
`tools/parity/out/m4_load_<tag>.json`. Замеры на RTX 3060 (KV q8_0, корпус
120 документов):

| `parallel`                                     | ctx/слот | коды | p50     | **p95**     | VRAM        |
| ------------------------------------------------ | ------------ | -------- | ------- | ----------------- | ----------- |
| 5                                                | 16384        | 200×5   | 25,7 с | **49,6 с** | 9669 МиБ |
| 3                                                | 16384        | 200×5   | 30,8 с | 48,9 с           | —          |
| 2                                                | 16384        | 200×5   | 42,3 с | 58,1 с           | —          |
| 5 (+`hds index --full` параллельно) | 16384        | 200×5   | 41,4 с | 315,1 с          | —          |

При `parallel 2/3` очередь реально работает: одновременно исполняется ≤2/3,
остальные ждут, отказов (`rejected_*`) и таймаутов нет.

**Приёмка SLA (M5 T0.1): не воспроизводилась — найден и устранён корень.**
Код-ревью M4 (F1) показало: повтор «чистого» прогона тоже давал p95 315 с
(3 отчёта из 5 за порогом). Три контрольных прогона до фикса:
`315,6 / 315,7 / 46,5 с` — выброс вероятностный (2 из 3). Root-cause по логам
резидента: **on-demand ПЕРЕзагрузка роли `embedding` при тесной VRAM** — fit-шаг
движка `llama_params_fit_impl` при «свободно ~1,9 ГиБ» завершается за **~299 с**
(замер f1_r1/r2: fit chat 0,57 с → embedding 4,2 с → rerank 27,3 с → embedding-
перезагрузка ~299 с; знак «−» в логе — баг печати движка). Unlucky-запрос ждал
перезагрузку целиком (315 с ≈ 299 fit + 16 генерация), очередь при этом
`granted=5, total_wait_ms=0` — стопор внутри движка, а не в очереди.

**Фикс (M5, сервер-профиль):** embedding/rerank по умолчанию `retention: keep`
(перезагрузок нет вовсе; явный `llm.<role>.retention` сильнее) + **прогрев**
embedding/rerank при старте `llm-host` (детерминированная часть старта, ~5–30 с,
вместо сюрприза под нагрузкой). Desktop не меняется (`load_on_demand` — экономия
VRAM). Приёмка после фикса — **3/3 прогона в SLA: 34,1 / 34,0 / 33,9 с**
(`m4_load_f1_fix_r1..r3.json`). Отдельный хвост: рост длительности fit-шага
движка от числа загрузок в процессе — кандидат в `engine-patch` (P5), не блокер.

### Внешний chat-provider `remote` (M5 T1–T4)

`chat.provider: remote` — RAG-ответ генерирует **внешний OpenAI-совместимый API**
(`chat.base_url`), а `llm-host` **не поднимает chat-роль** (её нет в плане
инстансов; embedding/rerank остаются локальными). Это «рычаг SLA»: чат-модель
Q6_K ~8,5 ГиБ VRAM освобождается — замер `m4_load_m5_remote_r1.json`
(5 × `ask_my_files`, фейковый remote 3 с): **VRAM 1102 → 1121 МиБ** (было
8916 → 9643), все 200, ответы с источниками; очередь/квоты работают без изменений.

```yaml
chat:
  provider: remote          # local (дефолт) | remote
  base_url: "https://api.example.com/v1"
  api_key: "sk-..."         # Bearer-ключ (только remote); "-" = без ключа; в git не входит
  model: "gpt-4o-mini"
```

* `Authorization: Bearer <chat.api_key>` — на каждый запрос к remote
  (непустой и не «-»); TLS-клиент — rustls (корни webpki), только исходящие:
  входящие серверы остаются без TLS (периметр = reverse-proxy, M3).
* `hds check` показывает пункт `chat_provider`: remote без `base_url`/`api_key`
  → warn; секрет в диагностике не печатается.
* ⚠ **Без анонимизации (W9, ещё не реализована)** текст вопроса и фрагменты
  локальных файлов уходят провайдеру как есть — включайте `remote` только для
  доверенных API.

*Чек-лист внешнего HTTPS (домен/сертификат, `/mcp` из Cline, `/ui` за прокси,
XFF-IP в аудите, `429` через прокси) выполняется на машине с прокси — вне
dev-стенда.*

### Несколько GPU, вендоров и пресеты раскладки (M6)

Прод-профиль — несколько **дискретных** GPU **одного семейства**:
NVIDIA под Windows **или** AMD под Windows (Vulkan), либо **один Mac** (Metal).
**Смешанные вендоры и iGPU/UMA не поддерживаются** — `hds check` (пункт `gpu_roles`)
даёт `warn` с числами и подсказкой.

Раскладка ролей по картам задаётся явно (`gpu.devices`) или **пресетом**:

```yaml
gpu:
  preset: nvidia-n      # none | nvidia-one | nvidia-n | amd-n | apple
  autodetect: false     # по умолчанию ВЫКЛЮЧЕН: раскладка только по явному ключу
  # devices:                                 # явная раскладка — сильнее пресета
  #   - { index: 1, roles: [chat] }
  #   - { index: 2, roles: [embedding, rerank] }
llm_server:
  #   chat: { device: 1 }                    # карта роли ключом — сильнее всего
```

Приоритет источников: `llm_server.<role>.device` → `gpu.devices` → `gpu.preset` →
`gpu.autodetect` → компат `gpu.device_index`. Индексы — **наши** (`0` = CPU,
`1..N` = ускорители в порядке `llm-host devices`; с Vulkan-сборкой порядок
отличается от CUDA — проверяйте `llm-host devices`).

* **Пресеты вендор-гомогенные** и раскладывают **уже объявленные** роли (реплики
  `chat-N`/`whisper-N` объявляются в конфиге явно): `nvidia-one`/`apple` — всё на
  первую карту (масштаб — `parallel` и/или реплика `chat-2`); `nvidia-n`/`amd-n` —
  чат-реплики и `whisper` round-robin по картам, индексация (`embedding`/`rerank`) —
  одним экземпляром на выделенной (последней) карте.
* **Правила (§3.5):** чат-реплики — одинаковые (одна модель/контекст, разные порты и
  слоты); `embedding`/`rerank` — **ровно один экземпляр** на машину; `whisper` —
  экземпляр на карту. Нарушение — **fail-fast** с числами (конфиг не запускается).
* **Один фасад:** наружу публикуется только `chat` → 8010; запросы раскидываются по
  репликам внутри хоста (least-busy → round-robin). Порты реплик — справочные.
* **Бюджет/вытеснение — по карте роли** (`llm_host status`, поле `cards`):
  «карта N (backend, устройство): свободно X МиБ, роли: …». Роли другой карты чужой
  нехваткой VRAM не вытесняются; индексация — «лёгкая добавка», чат-реплики не
  вытесняются.
* **Печатается с числами:** лог старта `llm-host` («раскладка ролей», «пресет»),
  `llm_host status`/`devices`, `llm_host_plan`, `hds check` (пункт `gpu_roles` —
  карты, роли, backend, свободная VRAM, уместный пресет). Секретов не печатаем.

Честные ограничения M6: живой проверки **несколько карт** нет (нет железа) —
мультикарточность принята юнитами и явным хвостом; предупреждение о **смешанных
вендорах/UMA** уже есть; **Apple Silicon** — подготовка без живой проверки;
транскрибация рядом с чатом на 12 ГБ уходит на GPU при `свободно ≥ модель+буфер+резерв`
и на CPU по бюджету иначе (числа — в логе); AMD-путь проверен частично (на стенде
только AMD **iGPU**); ⚠ известное ограничение: штатные клиенты транскрибации всегда
шлют поле `gpu` — host-выбор карты под файл срабатывает только при прямом
`POST /internal/transcribe` без `gpu` (см. `PLAN_M6` §8a, T5 / F-m6-t5-1).

## sidecar — Python-воркер извлечения

Автономный процесс `sidecar\hds_extract\worker.py` (stdio + JSON-RPC 2.0, NDJSON): `hello`,
`extract`, `normalize`, `clip_image` (CLIP в Rust — отвечает «не поддерживаю»), `shutdown`.
Ядро запускает его лениво и владеет процессом. Извлечение — PDF/DOCX/XLSX/PPTX/OCR и `.mpp`
(MS Project через mpxj/Java); лемматизация (`pymorphy3`) — для FTS.

**Самодостаточность.** `installers\build_sidecar.ps1` собирает дерево, работающее **без**
Python-ядра проекта: портативный CPython (python-build-standalone) + зависимости +
**копия** нужных модулей `hds\` под `sidecar\hds\`. Воркер кладёт `sidecar\` в `sys.path`
перед корнем проекта — в поставке `hds` берётся из копии, в разработке — из корня.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_sidecar.ps1 -OutDir dist\sidecar -SelfTest
```

## Индексная БД и файлы-сигналы

* **`index.db`** — SQLite: `files`, `chunks`, `chunks_fts` (FTS5, лемматизированный), `chunks_vec`
  (`sqlite-vec`, dim `embedding.dim`), `images_vec` (CLIP, dim 512). Путь — `db_path` (по умолчанию
  относительный — от корня проекта; в версионной раскладке лучше абсолютный или `data\index.db`).
* **`index.stop`** — файл-команда: аккуратно остановить текущую индексацию.
* ⚠️ **Схема изменилась (M1: `files.project`)** — старая база (без `project`) дропается
  при первом `connect()` (сообщение в stderr) — **переиндексация обязательна**.
* **`index.pause`** — приостановить индексацию (ставит UI/диспетчер; **пользовательскую не снимать сами**).
* **`index.heartbeat.json`** — кросс-процессный статус/прогресс (свежесть < 30 с).
* **`watch.lock`** — pid наблюдателя; **`data\llm-host.pid`** — pid резидента; логи — `data\logs\`.

## Серверный режим: доступ, службы, периметр, пакеты (M1–M3, M7–M9)

> Подробно и по сценариям — серверное руководство **`README_SERVER_MULTIUSER.md`**
> (единый источник: роли §1, сценарии A–F §2–§7, портрет отказов §8, этапы M7/M8/M9
> §8c–§8e, периметр и конкурентность §11). Ниже — только «что умеет» и ключи конфига.

| Этап     | Что умеет                                                                                                                                                                                                                                                                                             | Ключи конфига / команды                                                                                                                                                          | Где подробно |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------- |
| **M1** | доступ и проекты: проект — единица разграничения; логин/пароль (Argon2id) и токены (sha256, TTL 90 дн.); Bearer-гейт →`401`; фильтрация поиска/RAG по проектам; индексация — только админ | `access.enabled`, `access.token_ttl_days`; `index.roots` (имя проекта — строка `- "E:\Archive"` или `{ path, project }`); `hds user …`, `hds token …`           | §2–§3, §7           |
| **M2** | серверный профиль, каталог состояния, OS-службы, headless-UI                                                                                                                                                                                                            | `profile: desktop\|server`; `ui: { host, port }`; `hds service <role>` (`watch, mcp-http, ui, llm-host`)                                                                                     | §2a                    |
| **M3** | сетевой периметр: наружу только`/mcp` и `/ui` через TLS-прокси; аудит, rate-limit, hardening                                                                                                                                                                  | `trusted_proxy`; `audit.{enabled, max_mb}`; `rate_limit.{enabled, requests_per_min, burst, login_per_min, max_inflight, max_inflight_per_user, per_token}`; `http.{max_body_kb, max_conns}` | §11                    |
| **M7** | управление службами и выдача токенов из веб-UI (права админа)                                                                                                                                                                                                 | `GET/POST /api/services`; `hds token new --user <login> [--ttl-days N]`                                                                                                                         | §8c                    |
| **M8** | серверный пакет: установка одной (unattended) командой, обновление и откат                                                                                                                                                                                | `installers\install_server.ps1` (`-WhatIf`, `-Roles`, `-EnableAccess`, …) / `install_server.command`; `installers\update.ps1 -Profile server [-RollbackTo <ver>]`                      | §8d                    |
| **M9** | тонкий клиент: Cline к удалённому серверу, без движка/моделей/индекса                                                                                                                                                                                    | `hds cline-sync --server-url <url> --token <bearer> [--mcp-path /mcp]`; `setup-client.ps1`; `installers/build_client_release.ps1`                                                             | §8e                    |

⚠️ После M1 **переиндексация обязательна** (схема БД изменилась — см. «Индексная БД и
файлы-сигналы» выше). Без `access.enabled` (десктоп) всё работает как раньше: без токенов
и без фильтра.

## Настройки (`config.yaml`)

Полный образец — `config.example.yaml`. Основное:

* **`index`** — `roots` (корни), `exclude_dirs`/`exclude_paths`, `max_file_mb`/`max_media_mb`,
  `ocr`/`ocr_lang`/`ocr_tesseract_cmd`, `transcribe` + `whisper_*` + `transcribe_url` (ASR движком,
  владелец — llm-host), `clip`/`clip_*` (ONNX-модели; по умолчанию `models\clip_onnx`), `max_chunks`.
* **`db_path`** — путь к `index.db` (относительный — от корня проекта).
* **`chunk`** — `size`/`overlap` (структурный чанкер).
* **`embedding`** — `base_url` (`:8011/v1`), `model` (`text-embedding-bge-m3`), `batch_size`, `dim` (1024).
* **`chat`** — `base_url` (`:8010/v1`), `model`, `thinking` (`off|auto`), `temperature`,
  `max_tokens` (потолок вывода одного ответа; дефолт llm-host для клиентов без своего
  `max_tokens`), `rag_max_tokens` (отдельный, более короткий бюджет ответа `ask_my_files`),
  `max_context_chars`.
* **`llm_server`** — `host`, `autostart`, порты/модели/`ctx_per_slot`/`extra_args` по ролям
  (`chat`/`embedding`/`rerank`). Модель роли — `shared:<role>` (из общего рантайма) или путь.
* **`gpu`** — `policy`, `device_index` (0=CPU, 1=первый GPU), `n_gpu_layers`, `reserve_mb`,
  `evict_idle_sec`, `pause_index_on_query`, `priorities`.
* **`db`** — `busy_timeout_ms`, `read_only_conns`, `journal_mode` (M4 T1: server — 5000/wal/true,
  desktop — 30000/false; бэкап только `VACUUM INTO`/`.backup`).
* **`queue`** — `enabled`, `max_depth`, `wait_timeout_sec`, `policy` (M4 T3: очередь тяжёлых RAG,
  fair-share по пользователю; см. §«Слоты и SLA»).
* **`rate_limit`** — `enabled`, `requests_per_min`, `burst`, `login_per_min`, `max_inflight` (M3);
  `max_inflight_per_user`, `requests_per_min_per_user`, `per_token` (M4 T4 — квоты по пользователю;
  ключ лимитов — `user:<логин>`, иначе IP; см. §«Слоты и SLA»).
* **`http`** — `max_body_kb`, `max_conns`, `read_timeout_sec` (M3 T4).
* **`mcp_http`** — `host`/`port`/`path`/`autostart`/`start_timeout`.
* **`search`** — `vec_k`/`fts_k`/`rrf_k`/веса/`snippet_chars`.
* **`rerank`** — `enabled`/`url`/`model`/`timeout`/`max_latency`.
* **`watch`** — `debounce_seconds`, `reconcile_on_start`, `max_stable_wait`.
* **`extract`** (опц.) — `idle_timeout` воркера, сек.

Переменные окружения: `HDS_CONFIG` (путь к конфигу), `HDS_EXTRACT_PYTHON` (интерпретатор воркера),
`HDS_ENGINE_DIR` (каталог движка), `HDS_ROOT` (корень проекта).

## Разработка

Воркспейс — `crates/`:
`hds-core` (config/db/http/диагностика), `hds-extract` (клиент воркера), `hds-index`
(обход/хэш/чанкер/конвейер/watch/transcribe/**diag**), `hds-llama` (движок, фасад, `llm_host`),
`hds-clip` (ONNX vision+text), `hds-search` (fts/snippet/rerank/rag), `hds-mcp`, `hds-ui`,
`hds-cli` (бинарь `hds`).

```powershell
cargo build --release -p hds-cli -p hds-mcp -p hds-llama
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                     # 174 passed / 0 failed (+9 ignored)

# релизный пакет (плана §10.1). Резидент держит target\release\llm_host.exe ->
# либо -SkipBuild (готовые бинарники), либо предварительный `llm_host stop`.
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_rust_release.ps1 -Version 0.1.0 [-SkipBuild]

# портативный sidecar (нужен uv, ~280 МБ загрузки)
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_sidecar.ps1 -OutDir dist\sidecar -SelfTest
```

CI (`release.yml`): `test-rust` (fmt/clippy/tests) → `build-sidecar` → `build-windows`
(package + smoke) → `fetch-engine-runtime` → `release`.

## Патч движка (временно, до апстрима)

Ядро работает на движке `openresearchtools/engine` (v1.15). Мы несём **свой патч** движка
(`engine-patch/`), пока владелец не починит дефекты у себя: инцидент «загрузка роли держала
мьютекс инстанса и парализовала `status`, вытеснение и роли» — `tools/parity/W4_REPORT.md` §14.

* **Что в патче:** ограниченное ожидание слота (P1), загрузка модели **вне** мьютекса
  инстанса (P2 — причина инцидента), порядок блокировок (P3) и **тип KV-кэша**
  (`cache_type_k/v`: теперь работают `--cache-type-k q8_0 --cache-type-v q8_0` из `config.yaml`;
  для квантованного V патч включает Flash Attention).
* **Эффект (замер 02.10.2026, RTX 3060):** KV чата 512 → **272 МиБ**, VRAM чата
  7905 → **7717 МиБ**; compute-буфер embedding при `--ubatch-size 512` — 90 МиБ вместо ~1,4 ГиБ.
* **Установка/откат:** `installers\fetch_engine_runtime.ps1 -PatchEngine` /
  `-RollbackEnginePatch` (файлы и sha256 — `runtime-manifests/engine-patch.json`; штатные DLL
  сохраняются как `*.orig`). Наша сторона совместима со стоковым движком: поля KV добавлены в
  конец структур API и со стоковой DLL просто игнорируются.
* **Сборка из исходников** (если нужно воспроизвести): `engine-patch/README.md` — база — тег
  `v1.15`, тулчейн CUDA 13.4 + Ninja Multi-Config, staging подхватывает патч из `<repo>\bridge`.

## Диагностика

* `hds check` / в UI «Проверка компонентов» — БД, корни, роли chat/embedding, OCR, ffmpeg,
  лемматизатор (воркер), **`.mpp`** (mpxj), реранк, **`gpu-observability`** (L1: свежесть
  heartbeat резидента, занят ли движок и кем, «наш процесс / чужие» по VRAM — читает
  `data\llm-host.heartbeat.json`, поэтому работает и когда HTTP резидента молчит); отдельный
  пункт `gpu-manual` (whisper/Vulkan — проверяются вручную, GPU-чек-лист).
* `hds status` — состояние индекса; `hds whisper-check` — готовность ASR.

**Грабли.** `git` — всегда `--no-pager`. PowerShell 5.1 читает `.ps1` без BOM как ANSI — наши
скрипты ASCII-only; в строках писать `${var}`, а не `$var:` (последнее парсится как имя диска).
Кириллица в `Select-String` не ищется (ASCII-шаблон или чтение файла инструментом).

## Лицензии

Сторонние компоненты (движок openresearchtools, NVIDIA CUDA EULA, FFmpeg, Tesseract,
PyMuPDF — **AGPL-3.0**, mpxj/JPype + Java, модели) и их лицензии — `NOTICE.md`.

## Осталось в Python

По плану миграции в Python остаётся **только sidecar** (извлечение/лемматизация) — §2.5/§5.
Python-ядро (поиск/RAG/MCP/UI/индексатор/менеджер ролей) переписано на Rust и удалено;
Python-джобы выведены из CI. Паритет-эталон — `tools/parity/` (golden **заморожен**, регенерация
невозможна — генератор удалён); журналы волн — `tools/parity/W1_REPORT.md`…`W4_REPORT.md`, `STATUS.md`.
