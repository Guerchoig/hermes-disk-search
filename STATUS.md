# Состояние проекта hermes-disk-search — контекст для нового чата

> Рабочая папка: `C:\Users\Sasha\hermes-disk-search`
> GitHub: Guerchoig/hermes-disk-search (private), ветка main
> Версия: v0.2.0 (релиз выпущен через GitHub Actions)

## 1. ЗАДАЧА

Система хранения и поиска информации на локальных дисках по свободному запросу
на естественном языке. Работает поверх Hermes Agent Desktop (MCP) + LM Studio
(локальные модели на CUDA RTX 3060 12ГБ).

Пример: «Найди на моём компе, в каких проектах использовался 1С:Документооборот»
→ агент ищет по индексу и отвечает со ссылками на файлы (PDF, Office, Project,
видео с транскрипцией, картинки с OCR).

## 2. ПЛАН ДЕЙСТВИЙ (текущий спринт)

### Выполнено:
- ✅ Полная система: индексатор, watcher, поиск, RAG, MCP для Hermes, UI
- ✅ Релиз v0.2.0 через GitHub Actions (71 тест OK)
- ✅ ETA/скользящая скорость в heartbeat и статус-строке
- ✅ Деревья папок с раскраской в UI (none/partial/done)
- ✅ Переиндексация по пути из UI
- ✅ Параметры: transcribe, max_media_mb, max_chunks — редактируемые из UI
- ✅ Heartbeat-файл для кросс-процессного статуса
- ✅ chunk_count записывается в БД (был баг)
- ✅ indexed_at записывается при завершении + бэкфилл по mtime

### Выполнено в этом чате (завершение прерванного рефакторинга):
1. ✅ **Откат конвейера батч-эмбеддинга** в `hds/indexer.py`:
   - удалены `pending`, `bs_threshold`, `_flush_pending()` из `run_index`;
   - тело цикла заменено на `status, kind2 = process_file(conn, emb, cfg, path,
     force=full, progress_cb=progress_cb)` — обработка по одному файлу, надёжно;
   - `_extract_file` (фаза 1) + `_commit_file` (фаза 2) сохранены, `process_file`
     вызывает их с правильным порядком аргументов (раньше был перепутан — баг).
2. ✅ **Исправлен дедлок в `hds/progress.py`** — причина висящих тестов:
   `heartbeat_data()` и `_status_line()` держали `self._lock` и вызывали
   `rate_window()`/`eta_sec()` (тот же замок) → deadlock на каждом `_hb()`.
   Замок заменён на `threading.RLock()`.
3. ✅ Инициализация `self._seen_ts` (deque) и `self.total` в `__init__`
   (раньше `AttributeError` при heartbeat без пре-подсчёта).
4. ✅ Кодировочно-устойчивый вывод (`ProgressReporter._write`): строка статуса
   (⏱ и т.п.) перекодируется с replace для cp1251-консоли/pipe — раньше
   `UnicodeEncodeError` ронял `run_index` в `rep.finish()` (в т.ч. в фоновом
   потоке теста heartbeat, из-за чего `res == {}`).
5. ✅ **Все 71 тест зелёные** (`python -m unittest discover -s tests` — OK, ~4 с).
6. ✅ `python -m hds.cli check`: sqlite-vec, LM Studio (bge-m3 отвечает),
   ffmpeg, faster-whisper — OK. Опциональные Tesseract/mpxj не установлены
   (известно, некритично).

### НЕ выполнено (следующий шаг):
1. **Релиз v0.2.1 по releasing.md**: обновить `__version__` в `hds/__init__.py`,
   создать `RELEASE_NOTES_v0.2.1.md`, закоммитить/запушить, 
   `gh workflow run release.yml -f version=v0.2.1`.
   Перед выпуском — ручная проверка «спроси в Hermes → ответ со ссылками»
   и `check` (пройден локально, см. выше).

## 3. ПРОБЛЕМЫ И БАГИ

### РЕШЁННЫЕ (все в main)
1. os.kill(pid,0) на Windows УБИВАЕТ — ctypes OpenProcess
2. mcp 2.x FastMCP→MCPServer — обе версии поддержаны
3. PS 5.1 + кириллица .ps1 без BOM — все .ps1 с BOM
4. re.sub с Windows-путём в replacement — lambda-подстановка
5. Предупреждения HuggingFace подавлены, русские сообщения
6. Windows-путь в YAML двойных кавычках — одинарные кавычки
7. indexed_at не записывался → записывается + бэкфилл mtime
8. chunk_count не записывался → записывается в finish_file
9. Не начатые папки показывались как partial → fresh-проверка на NULL iat
10. fts_query игнорировал однобуквенные слова → тест исправлен
11. Node 20 deprecation в Actions → checkout@v5, setup-python@v6
12. Make_icon print кириллица падала на runner → utf-8 reconfigure

### АКТИВНЫЕ ПРОБЛЕМЫ
1. UI-статусы бейджей могут врать (heartbeat удаляется при завершении;
   возможное решение — показывать last_reporter-снапшот с пометкой «завершён»).
2. Опционально: аккуратная оптимизация батч-эмбеддинга (можно позже, с полным
   тестовым покрытием; базовый путь через _commit_file уже батчит внутри файла).

### РЕШЁННЫЕ В ЭТОМ ЧАТЕ
13. Дедлок в ProgressReporter: не-RLock + вложенные rate_window/eta_sec
    под замком — висели ~20 тестов (heartbeat, пауза, стоп)
14. `_seen_ts`/`total` без инициализации в __init__ → AttributeError
15. UnicodeEncodeError (⏱ в cp1251-консоли/pipe) ронял run_index в rep.finish()
16. process_file вызывал _commit_file с перепутанным порядком аргументов

---

## 4. ТЕКУЩЕЕ СОСТОЯНИЕ ФАЙЛОВ

### hds/indexer.py — РАБОТАЕТ (после отката конвейера)
- process_file = _extract_file (фаза 1: проверки+извлечение+чанки)
  → _commit_file (фаза 2: эмбеддинги+запись), возвращает (статус, kind)
- run_index: обход, пауза/стоп-файлы, heartbeat (_hb), пре-подсчёт total
  в daemon-потоке (ETA), prune с защитой от массового удаления;
  конвейер pending/_flush_pending удалён
- path_excluded, content_hash, iter_files, reindex_path, _prune_deleted — ок

### hds/progress.py — РАБОТАЕТ
- RLock (вложенные rate_window/eta_sec безопасны), _seen_ts/total в __init__
- _write(): кодировочно-устойчивая печать (cp1251/pipe не падают)
- _fmt_eta, rate_window(300), eta_sec, heartbeat_data (total/eta/remaining)
- _status_line включает ETA

### hds/ui_server.py — БЫЛ ПОВРЕЖЁН, ВОССТАНОВЛЕН
- Проверить py_compile + ast.parse перед использованием!
- _index_state (репортёр + heartbeat + last_reporter), _db_stats
- _build_trees (данные + агрегация), _set_simple_config, _save_config
- _start_index, _db_move, _watch_start/stop/autostart_set

### hds/db.py — РАБОТАЕТ
- finish_file(status, error, chunks) — записывает indexed_at + chunk_count
- connect(): sqlite-vec + FTS5 + vec_dim миграция + backfill + busy_timeout
- upsert_file, add_chunk, add_vector, get_file_by_path/hash
- rename_path, remove_path, stats

### hds/dbops.py — РАБОТАЕТ
- move_db(): атомарный перенос БД через psutil, kill_processes параметр

### Остальные файлы — РАБОТАЮТ
- hds/watcher.py, search.py, rag.py, extractors.py, extract_static.py,
  extract_av.py, chunker.py, embedder.py, mcp_server.py, cli.py, config.py
- assets/ui.html, tools/make_icon.py, gen_fixtures.py, test_mcp.py

### config.yaml
- db_path = D:\hermes-disk-search-db\index.db
- roots = ["D:\\"], max_media_mb: 2500, max_chunks: 3000, transcribe: true
- exclude_dirs включает $RECYCLE.BIN, .Trash, .Trashes, hermes-disk-search-db

---

## 5. ЧТО ПРЕДСТОИТ СДЕЛАТЬ

### КРИТИЧНО (следующий шаг)
1. Релиз v0.2.1 по releasing.md (тесты зелёные, check пройден):
   `__version__` в hds/__init__.py → 0.2.1, RELEASE_NOTES_v0.2.1.md,
   commit+push, `gh workflow run release.yml -f version=v0.2.1`
2. Ручная проверка сценария «спроси в Hermes → ответ со ссылками»

### ВАЖНО (после релиза)
3. UI-бейджи: не показывать «идёт индексация» по устаревшему heartbeat —
   показывать last_reporter-снапшот с пометкой «завершён»
4. Обновить README раздел про ETA (если требуется)

### ЖЕЛАТЕЛЬНО
5. Оптимизация батч-эмбеддинга (аккуратно, с полным тестовым покрытием —
   прошлая попытка вызвала каскад багов, откатила см. раздел 2)
6. UI-кнопка для запуска watcher'а с новыми настройками
7. Поддержка macOS-инсталлятора в тестах

---

## 6. КОМАНДЫ ДЛЯ БЫСТРОГО СТАРТА

```powershell
cd C:\Users\Sasha\hermes-disk-search
.\.venv\Scripts\python.exe -m hds.cli check      # диагностика
.\.venv\Scripts\python.exe -m unittest discover -s tests  # тесты
.\.venv\Scripts\python.exe -m hds.cli status     # состояние индекса
# UI: ярлык «Hermes Disk Search» на рабочем столе
# LM Studio: localhost:1234 (text-embedding-bge-m3 + qwen3.5-9b@q6_k)
# Hermes MCP: disk-search (в config.yaml Hermes Desktop)
```

### Ключевые технические детали
- HDS_CONFIG env переопределяет путь к config.yaml
- db_abs_path(cfg) разрешает db_path относительно PROJECT_ROOT
- indexer._ACTIVE_REPORTER — текущий репортёр (для UI/MCP статуса)
- indexer._LAST_REPORTER — снапшот последнего завершённого прогона
- index.heartbeat.json — кросс-процессный статус (ts свежесть < 30с)
- index.stop, index.pause, watch.lock — файлы-сигналы
- WAL-режим SQLite: параллельная запись безопасна (busy_timeout=5000)
- config.yaml — одинарные кавычки YAML для Windows-путей (не двойные!)
- ui_server.py восстанавливался после повреждения — при сомнении прогоните
  py_compile + test_ui_server
- Тесты импортируют helpers из папки tests/ → запускать discover или из tests/
