# hermes-disk-search

Система хранения и поиска информации по вашим дискам со свободным запросом на естественном языке.
Работает поверх **Hermes Agent Desktop** + **LM Studio** (или Ollama) на CUDA-видеокарте.

Пример вопроса в чате Hermes: *«Найди на моём компе, в каких проектах использовался 1С:Документооборот»* —
ответ придёт со ссылками на локальные файлы (тексты, PDF, MS Office, MS Project, видео, картинки)
с указанием страниц и таймкодов.

## Как это работает

```
Индексатор                Хранилище                Доступ
обход дисков (D:\)   ->   SQLite + sqlite-vec   ->  MCP-сервер (чат Hermes)
PDF/DOCX/XLSX/PPTX      + FTS5 (ключевые слова)   поиск_local_files / ask_my_files
MS Project .mpp         векторы bge-m3            CLI: python -m hds.cli search|ask
картинки: OCR (Tesseract)                         watcher: мгновенная реакция
аудио/видео: faster-whisper (CUDA)                на события файловой системы
```

Гибридный поиск: семантический (векторы) + ключевой (FTS5/BM25), слияние RRF.
Если модель эмбеддингов недоступна — поиск деградирует к ключевым словам, система продолжает работать.

## Установка

```powershell
cd C:\Users\Sasha\hermes-disk-search
.\setup.ps1
```

Что нужно дополнительно:
1. **Embedding-модель `bge-m3`** — установлена (GGUF-версия `lm-kit/bge-m3-gguf`, Q8_0, 1024 dim).
   Если понадобится переустановить: `lms get` часто не находит модель — скачайте файл вручную:
   `curl.exe -L -o "%USERPROFILE%\.lmstudio\models\lm-kit\bge-m3-gguf\bge-m3-Q8_0.gguf"
   https://huggingface.co/lm-kit/bge-m3-gguf/resolve/main/bge-m3-Q8_0.gguf`,
   затем загрузите модель в LM Studio (или `lms load text-embedding-bge-m3 -y`).
2. **Tesseract OCR** (текст на картинках/сканах): `winget install UB-Mannheim.TesseractOCR`
   (+ пакет русского языка), путь к tesseract.exe — в `config.yaml: index.ocr_tesseract_cmd`, если не в PATH.
3. **faster-whisper** (транскрипция аудио/видео на CUDA) — ставится setup.ps1; ffmpeg должен быть в PATH.
4. **MS Project (.mpp)** — опционально: `pip install mpxj` (в venv) + Java 11+.

Диагностика: `.venv\Scripts\python.exe -m hds.cli check`

## Использование

```powershell
# Первичная индексация (запустить на ночь; прогресс в консоли)
.venv\Scripts\python.exe -m hds.cli index

# Быстрый поиск
.venv\Scripts\python.exe -m hds.cli search "в каких проектах использовался 1С:Документооборот"

# Развёрнутый ответ с цитатами (RAG через локальную модель)
.venv\Scripts\python.exe -m hds.cli ask "в каких проектах использовался 1С:Документооборот"

# Состояние индекса
.venv\Scripts\python.exe -m hds.cli status

# Наблюдатель: мгновенная индексация по событиям ФС (создание/изменение/удаление/переименование)
.venv\Scripts\python.exe -m hds.cli watch

# Переиндексировать файл/папку; убрать из индекса
.venv\Scripts\python.exe -m hds.cli reindex "D:\путь"
.venv\Scripts\python.exe -m hds.cli forget "D:\путь\файл.pdf"
```

### Автозапуск наблюдателя (Windows)
`.\install_autostart.ps1` — создаст задачу Планировщика `HermesDiskSearchWatch` (при входе в систему).
На macOS — LaunchAgent (plist, `/Library/LaunchAgents/`), команда та же: `python -m hds.cli watch`.

### Расписание вместо наблюдателя
Можно не держать watcher, а индексировать по cron Hermes или Планировщику: ночная команда
`python -m hds.cli index` (инкрементальная, быстро: только новые/изменённые файлы).

## Безопасная остановка индексации

Индексацию можно прервать в любой момент без ущерба:

- **Ctrl+C** в консоли — аккуратное завершение: обработанные файлы уже сохранены
  (каждый файл коммитится атомарно), текущий файл откатится и будет дообработан
  при следующем запуске;
- **CLI**: `python -m hds.cli stop` — создаёт стоп-сигнал `index.stop`; удобно,
  когда индексация идёт в другом окне/фоне;
- **Из чата Hermes**: инструмент `stop_indexing` (то же самое);
- после остановки стоп-файл удаляется автоматически; `index --full`/`index` продолжат
  с места остановки (уже проиндексированное пропускается).

## Иконка, ярлыки, инсталляторы

- **Иконка**: `assets/icon.png` (512px), `assets/icon.ico` (Windows), `assets/icon.icns` (macOS).
  Генерируется программно: `python tools/make_icon.py` (можно заменить свою — просто подмените файлы).
- **Ярлык запуска индексации**:
  - Windows: `shortcuts\windows\create_shortcut.ps1` — создаёт «Индексация дисков.lnk»
    (на рабочем столе) с иконкой; целевой скрипт — `run_index.ps1` в корне проекта.
  - macOS: `shortcuts/macos/Индексация дисков.command` (двойной клик из Finder;
    после клонирования выполните `chmod +x`) и приложение
    `shortcuts/macos/HermesDiskSearchIndex.app` (иконка внутри, копируется инсталлятором в ~/Applications).
- **Инсталляторы** (проверяют и доустанавливают недостающее; LM Studio и Hermes
  не устанавливают — предупреждают и дают ссылки):
  - Windows: `installers\install_windows.ps1` (Python, ffmpeg, Tesseract по желанию, venv,
    ярлыки, автозапуск watcher по выбору).
  - macOS: `installers/install_macos.command` (brew, python3, ffmpeg, tesseract-lang, venv,
    установка .app).

## Управление моделями LM Studio (память GPU)

Код проекта **не управляет загрузкой моделей** — он только шлёт HTTP-запросы к `localhost:1234`
(эмбеддинги → `/v1/embeddings`, ответы → `/v1/chat/completions`). Загрузкой/выгрузкой ведает LM Studio:

- модели, загруженные вручную (`lms load ... -y` или кнопкой в UI) — живут постоянно, пока вы их
  не выгрузите; TTL и Auto-Evict их не трогают;
- JIT-загрузка (по первому запросу к модели) поднимает модель сама; TTL простоя по умолчанию 60 мин;
  настройка **Auto-Evict** («Only keep last JIT loaded model», включена по умолчанию) вытесняет
  предыдущую JIT-модель — если у вас чат-модель поднимается через JIT, **выключите Auto-Evict**,
  иначе чат и embedding будут выгружать друг друга;
- обе модели (разных типов) могут быть загружены одновременно; при нехватке VRAM LM Studio сама
  выносит лишние слои на CPU RAM — работает медленнее, но без сбоев.

Проверка текущего состояния: `lms ps` (колонка DEVICE покажет, что ушло на CPU).

Рекомендуемая схема при «чат-модель занимает почти всю VRAM»:
1. Держать обе модели загруженными постоянно: `lms load qwen3.5-9b@q6_k -y` и
   `lms load text-embedding-bge-m3 -y` — embedding-модель занимает всего ~700 МБ VRAM.
2. Если VRAM впритык — задать bge-m3 GPU offload = 0 слоёв (только CPU): для эмбеддингов это почти
   незаметно (модель маленькая, батч 32 фрагмента).
3. На время ночной индексации чат-модель можно выгрузить (`lms unload qwen3.5-9b@q6_k`), чтобы
   всё место досталось GPU: индексация требует только эмбеддинги.

## Интеграция с Hermes

В `config.yaml` Hermes добавлен MCP-сервер:

```yaml
mcp_servers:
  disk-search:
    command: C:\Users\Sasha\hermes-disk-search\.venv\Scripts\python.exe
    args:
      - C:\Users\Sasha\hermes-disk-search\mcp_start.py
    timeout: 300
```

Skill `disk-search` (в `skills\research\disk-search\SKILL.md` Hermes) объясняет агенту инструменты:
`search_local_files`, `ask_my_files`, `index_status`, `start_indexing`, `reindex_path`.
Перезапустите Hermes Desktop после установки.

## Конфигурация (`config.yaml`)

- `index.roots` — что индексировать (по умолчанию весь `D:\`)
- `index.exclude_dirs` — исключения; `max_file_mb` / `max_media_mb` — лимиты размеров
- `index.ocr` / `index.transcribe` — включить/выключить OCR и транскрипцию
- `embedding.*` — LM Studio, модель `bge-m3` (dim 1024)
- `chat.*` — модель ответов; `watch.*` — debounce наблюдателя

## Кроссплатформенность (Windows / macOS)

- Windows: события ФС через ReadDirectoryChangesW, автозапуск — Планировщик задач.
- macOS: события через FSEvents (`brew install ffmpeg tesseract-lang`), Whisper работает на CPU/Metal,
  автозапуск через LaunchAgent.
- Хранилище и поиск полностью кроссплатформенные (SQLite + sqlite-vec + FTS5).

## Защита от ложных удалений

Если диск был отключён и при обходе «пропало» >20% файлов, очистка индекса блокируется
до подтверждения (`index --confirm-delete`).

## Файлы проекта

| Путь | Назначение |
|---|---|
| `hds/indexer.py` | инкрементальный обход, хэши, prune |
| `hds/watcher.py` | события ФС (watchdog), debounce, догон при старте |
| `hds/extractors.py`, `extract_static.py`, `extract_av.py` | PDF/Office/текст; MPP/картинки; аудио/видео |
| `hds/db.py` | SQLite: files/chunks + FTS5 + vec0 |
| `hds/embedder.py` | клиент /v1/embeddings (LM Studio/Ollama) |
| `hds/search.py` | гибридный поиск RRF + сниппеты |
| `hds/rag.py` | ответ с цитатами через чат-модель |
| `hds/cli.py` | CLI |
| `hds/mcp_server.py`, `mcp_start.py` | MCP-сервер для Hermes |
| `gen_fixtures.py` | тестовые файлы для smoke-теста |