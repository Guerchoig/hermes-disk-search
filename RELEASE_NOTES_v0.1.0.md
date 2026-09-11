# hermes-disk-search v0.1.0

Локальный поиск по вашим дискам со свободным запросом на естественном языке.
Работает поверх Hermes Agent Desktop + LM Studio на CUDA-видеокарте.

Пример: *«Найди на моём компе, в каких проектах использовался 1С:Документооборот»* —
ответ со ссылками на файлы (тексты, PDF, MS Office, MS Project, картинки с OCR,
аудио/видео с транскрипцией), с указанием страниц и таймкодов.

## Установка
- **Windows**: запустите `installers\install_windows.ps1` — доустановит Python/ffmpeg/Tesseract,
  создаст venv, ярлык «Индексация дисков» с иконкой (по выбору — автозапуск наблюдателя).
- **macOS**: запустите `installers/install_macos.command` — brew-компоненты, venv, установка
  приложения «HDS Индексация» в ~/Applications.

Требуется установить вручную: [Hermes Agent Desktop](https://hermes-agent.nousresearch.com)
и [LM Studio](https://lmstudio.ai) (сервер на `localhost:1234`, модели: чат + embedding
`text-embedding-bge-m3`).

## Что внутри
- Гибридный поиск: векторы bge-m3 (1024) + полнотекст FTS5/BM25, слияние RRF
- Инкрементальная индексация; наблюдатель файловой системы (ReadDirectoryChangesW / FSEvents)
- Извлечение: PDF, DOCX, XLSX, PPTX, MS Project (.mpp через mpxj), OCR картинок (Tesseract),
  транскрипция аудио/видео (faster-whisper на CUDA)
- MCP-сервер для Hermes: `search_local_files`, `ask_my_files`, `index_status`, `start_indexing`, `reindex_path`
- SQLite-хранилище (WAL), кроссплатформенно: Windows / macOS

## Ассеты
- `icon.png` / `icon.ico` — иконка проекта (заменяйте на свою через `tools/make_icon.py`)
- `HermesDiskSearchIndex.app.zip` — macOS-приложение запуска индексации (иконка внутри)
- `hds-icons.zip` — все форматы иконки

**Полная документация**: [README.md](https://github.com/Guerchoig/hermes-disk-search#readme)