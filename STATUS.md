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

### НЕ выполнено (контекстное окно закончилось):
1. **Тесты виснут** — ~20 из 71 падают/зависают после рефакторинга конвейера.
2. **Конвейер батч-эмбеддинга** в run_index — недоделан, вызвал каскад багов.

---

## 2a. ПЛАН НА СЛЕДУЮЩИЙ ЧАТ

1. Откатить конвейер батч-эмбеддинга в `hds/indexer.py`:
   - В `run_index` УДАЛИТЬ: `pending = []`, `bs_threshold`, `_flush_pending()`,
     и логику if/elif/else с `_flush_pending()` в теле цикла.
   - ЗАМЕНИТЬ блок с res/fid/chunks на: `status, kind2 = process_file(conn, emb, cfg, path, force=full, progress_cb=progress_cb)`
   - ОСТАВИТЬ: `_extract_file`, `_commit_file`, `process_file` (рефакторинг ок)
   - ОСТАВИТЬ: ETA/скользящую скорость (в progress.py и heartbeat)
   - Убрать `pending = []` и `bs_threshold` из переменных
2. Исправить тесты по одному:
   - `test_stop_file_stops_gracefully` — виснет из-за пре-подсчёта
   - `test_pause_file_resumes` — аналогично
   - `test_heartbeat_written_during_and_removed_after` — виснет
   - `test_moved_reuses_chunks` — fail из-за chunk_count (уже исправлен код)
3. Прогнать все 71 тест — OK
4. Релиз v0.2.1 по releasing.md

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
1. Конвейер батч-эмбеддинга в run_index — каскад багов (тесты виснут)
2. ~20 тестов падают/виснут после конвейера + пре-подсчёта
3. UI-статусы бейджей могут врать (heartbeat удаляется при завершении)

---

## 4. ТЕКУЩЕЕ СОСТОЯНИЕ ФАЙЛОВ

### hds/indexer.py — ИЗМЕНЁН, ЧАСТИЧНО СЛОМАН
- process_file разбит на _extract_file (фаза 1) + _commit_file (фаза 2)
- run_index содержит конвейер с pending/_flush_pending — СЛОМАН, нужно откатить
- ETA/heartbeat/_hb() — работают
- path_excluded, content_hash, iter_files, reindex_path, _prune_deleted — работают

### hds/progress.py — РАБОТАЕТ
- _fmt_eta, _seen_ts deque, set_total, rate_window(300), eta_sec
- heartbeat_data включает total/eta_sec/rate_window/remaining
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

### КРИТИЧНО (следующий чат)
1. В hds/indexer.py run_index: откатить конвейер (pending/_flush_pending/
   bs_threshold) — заменить блок с _extract_file/commit на process_file
2. Убрать пре-подсчёт из синхронного пути (запустить в daemon-потоке)
3. Прогнать тесты, исправить оставшиеся
4. Убедиться что heartbeat и ETA работают
5. Релиз v0.2.1 по releasing.md

### ВАЖНО (после тестов)
6. Обновить README раздел про ETA
7. Добавить тесты для ETA/скользящей скорости

### ЖЕЛАТЕЛЬНО
8. Оптимизация батч-эмбеддинга (аккуратно, с полным тестовым покрытием)
9. UI-кнопка для запуска watcher'а с новыми настройками
10. Поддержка macOS-инсталлятора в тестах

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
- ui_server.py был повреждён множественными правками — ПРОВЕРЬТЕ ЦЕЛОСТНОСТЬ
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
1. Конвейер батч-эмбеддинга в run_index — каскад багов (тесты виснут)
2. ~20 тестов падают/виснут после конвейера + пре-подсчёта
3. UI-статусы бейджей могут врать (heartbeat удаляется при завершении)

---

## 4. ТЕКУЩЕЕ СОСТОЯНИЕ ФАЙЛОВ

### hds/indexer.py — ИЗМЕНЁН, ЧАСТИЧНО СЛОМАН
- process_file разбит на _extract_file (фаза 1) + _commit_file (фаза 2)
- run_index содержит конвейер с pending/_flush_pending — СЛОМАН, нужно откатить
- ETA/heartbeat/_hb() — работают
- path_excluded, content_hash, iter_files, reindex_path, _prune_deleted — работают

### hds/progress.py — РАБОТАЕТ
- _fmt_eta, _seen_ts deque, set_total, rate_window(300), eta_sec
- heartbeat_data включает total/eta_sec/rate_window/remaining
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
- roots = ["D:\\\"], max_media_mb: 2500, max_chunks: 3000, transcribe: true
- exclude_dirs включает $RECYCLE.BIN, .Trash, .Trashes, hermes-disk-search-db
