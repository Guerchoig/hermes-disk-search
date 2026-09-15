# Состояние проекта hermes-disk-search — контекст для нового чата

> Рабочая папка: `C:\Users\Sasha\hermes-disk-search`
> GitHub: Guerchoig/hermes-disk-search (private), ветка main
> Версия: v0.3.0 (релиз выпущен через GitHub Actions, run 34935328484 — success)

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
   (⏱ и т.п.) перекодируется с replace, если кодировка потока
   не поддерживает юникод (cp1251-консоль/pipe) — раньше `UnicodeEncodeError`
   ронял `run_index` в `rep.finish()` (в т.ч. в фоновом потоке теста heartbeat,
   из-за чего `res == {}`).
5. ✅ Все 71 тест зелёные (`python -m unittest discover -s tests`).

### Исправлено в этом чате (инцидент «UI не запускается, watcher умер»):
1. ✅ **Корневая причина падения watcher'а**: при старте reconcile-сверка
   (run_index по D:\) дошла до служебного lock-файла Office
   `~$Регистр НСИ.xlsx`, openpyxl бросил `BadZipFile: File is not a zip file`,
   исключение никто не перехватил → `process_file` → `run_index` →
   `run_watch` — процесс watcher'а умер. Отсюда: stale `watch.lock`
   (PID мёртв), «watcher не запущен», прерванная на середине сверка.
2. ✅ `hds/indexer.py::_extract_file`:
   - lock-файлы Office (`~$*`) пропускаются сразу (статус skipped_type);
   - извлечение обёрнуто в try/except: битый файл → `finish_file(status='error')`
     и статус `error: ...`, прогон индексации НЕ останавливается.
3. ✅ `hds/watcher.py::run_watch`: reconcile обёрнут в try/except — падение
   сверки не убивает наблюдателя (события ФС важнее).
4. ✅ `hds/ui_server.py::run`: перед bind пробуем подключиться к порту —
   если кто-то уже слушает, второй экземпляр молча выходит (на Windows
   SO_REUSEADDR позволял двум серверам молча делить порт 8765 — из-за
   зависшего старого сервера браузер открывал мёртвую страницу).
5. ✅ Регрессионные тесты (+3): lock-файл пропускается, битый xlsx →
   статус error (не падение), run_index доходит до конца с битым файлом.
   Итого 74 теста — все зелёные.
6. ✅ Восстановлены процессы: watcher (PID в watch.lock живой, сверка идёт,
   w_err.log чистый) и UI (api/status отвечает). Найдена причина падения
   watcher'а ранее — «forrtl: window-CLOSE event» в старом w.log (окно закрыли).
7. ✅ Автозапуск watcher: задачи HermesDiskSearchWatch в Планировщике НЕ было
   (не установлена / нет прав администратора). Установлен ярлык
   `HermesDiskSearchWatch.lnk` в папку автозагрузки (запуск при входе).
8. ✅ Диагностическая заметка: `.venv\Scripts\pythonw.exe` — шим, порождающий
   реальный `C:\Python314\pythonw.exe`, поэтому каждый сервис = ДВА процесса
   с одинаковой командной строкой (не дубликат!); пары процессов с одинаковым
   CommandLine — норма. Проверять liveness по watch.lock через _pid_alive.
9. ⚠️ Дисковый шум при старте watcher — норма: reconcile-сверка читает
   начало/конец всех файлов диска (content_hash); на ~205k файлов —
   минуты-часы, видно в w.log (ETA в статусной строке).

### Выполнено в этом чате (ускорение индексации + exclude_paths в UI):
1. ✅ **Найдено и измерено узкое место**: каждый HTTP-запрос к LM Studio открывал
   новое TCP-соединение ценой ~2 с (keep-alive-запрос — 0.01 с). GPU простаивал:
   ~6 chunks/s фактических против 82-147 chunks/s при батчах по живому соединению.
2. ✅ **`hds/embedder.py`: requests.Session (keep-alive)** — соединение
   переиспользуется; ускоряет и индексацию, и каждый поисковый запрос.
   Факт после фикса: 63 чанка за 1.9 с, типичный файл 0.1-0.4 с (было 2+ с).
3. ✅ **`embedding.batch_size` 32 → 64** (config.yaml, README, дефолт make_embedder).
4. ✅ **Новая настройка `index.exclude_paths`** (исключаемые пути-префиксы):
   - `indexer.py`: `_excluded_prefixes`/`_prefix_excluded` (нормализация
     регистра/слэшей, граница префикса по компоненту пути: `D:\Backup2` ≠
     `D:\Backup`); `iter_files` отсекает поддеревья целиком + одиночные файлы;
     `path_excluded` проверяет и dirs, и paths (покрывает watcher и process_file);
   - `ui_server.py`: `_set_exclude_paths` (нормализация, дедуп, сохранение в
     YAML с сохранением комментариев, блочный и inline-вид, атомарная запись,
     проверка итогового YAML, предупреждение при исключении корня) +
     эндпоинт `POST /api/config/excludes` + `exclude_paths` в `/api/status`;
   - `assets/ui.html`: textarea «Исключённые из индексации пути» в
     «Параметрах обработки» + кнопка сохранения (fill/save из refresh());
   - `config.yaml`: ключ `exclude_paths: []` + закомментированный пример.
5. ✅ Тесты +9 (префикс/граница, отсечение поддерева/файла, process_file,
   вставка/замена блока YAML, inline `[]`, пустой список, отказ не-списка,
   дедуп) — **итого 83, все зелёные**.
6. ✅ Сервисы перезапущены с новым кодом; замер: индексация ~30-50× быстрее
   (файл 0.1-0.4 с вместо 2+ с), скорость обхода 48k файлов/мин.

### Исправлено дополнительно (кнопки «Остановить watcher» / перезапуск):
1. ✅ `_hds_pids` (поиск процессов watcher'а) переписан на psutil: вызов
   PowerShell Get-CimInstance через subprocess ломался кавычками WQL
   («Invalid query») и **тихо возвращал пустой список** — taskkill никогда
   не запускался, кнопка «Остановить watcher» не работала.
2. ✅ `_watch_stop`: taskkill → psutil kill (TerminateProcess) + kill детей
   (shim → реальный интерпретатор) + ожидание фактического завершения.
3. ✅ `_watch_running` и `_acquire_lock` больше не доверяют watch.lock:
   PID из lock мог быть переиспользован ОС → «watcher уже запущен» при
   мёртвом watcher'е (старт молча не срабатывал). Теперь проверяется
   командная строка живого процесса ('hds.cli watch').
4. ✅ Живой цикл проверен через API: старт → процессы найдены → стоп
   (оба PID убиты, статус False) → повторный старт (работает).
5. ✅ Тесты +3 (_hds_pids находит процесс по cmdline; замена блока
   exclude_paths не съедает соседние ключи YAML) — **итого 86, все зелёные**.

### Выполнено дополнительно (интеграция с Hermes Desktop):
1. ✅ Диагностика «агент не использует MCP»: в сессии 13-14.09 агент на все 5
   запросов использовал search_files/terminal вместо disk-search (лог agent.log);
   причина — новая сессия (history=0), модель выбирала встроенные инструменты,
   наши описания были слишком скромными.
2. ✅ Усилены описания MCP-инструментов (search_local_files = «ГЛАВНЫЙ инструмент
   для "найди на этом компе…"», подсказки kinds=media/image/mpp).
3. ✅ Скилл `disk-search` в Hermes (skills\disk-search\SKILL.md): правило «любые
   "найди на компе" → MCP disk-search, не grep/terminal» + подсказки по kinds.
4. ✅ `install_hermes.ps1` — подключение disk-search к Hermes в один запуск
   (MCP-блок в config.yaml с сохранением комментариев + валидация YAML + скилл);
   идемпотентен; работает и когда Hermes установлен позже disk-search.
5. ✅ `setup.ps1` вызывает install_hermes.ps1 автоматически (последний шаг).
6. ✅ Скилл хранится в репозитории: hermes-skill\SKILL.md (источник для установки).
7. ✅ README: раздел «Интеграция с Hermes» переработан (авто/позже/вручную/проверка).

### Выполнено дополнительно (CLIP: поиск картинок по содержанию):
1. ✅ Вопрос пользователя: «удастся ли найти изображения по описанию содержания
   ("живые цветы")?» — проверено: НЕТ (чанк картинки = размеры+EXIF+OCR-текст;
   запрос «цветок» давал 0 результатов).
2. ✅ Быстрый фикс: в чанк картинки добавлено «папка: <имя>» (фото из папки
   «Цветы» находятся по названию папки).
3. ✅ **CLIP-индекс**: hds/clip_index.py (image-энкодер clip-ViT-B-32 +
   text-энкодер multilingual-v1 — понимает русский; 512-dim; CPU);
   таблица images_vec в БД; хук в run_index (CLIP-вектор при индексации
   каждой картинки); поиск: при kinds=image (или без фильтра) текстовый
   запрос → images_vec → RRF в общий рейтинг.
4. ✅ CLI `clip-index`: дозаполнение векторов по всем картинкам
   (busy_timeout 30 с + ретраи — параллельная запись watcher'а);
   vec0 не поддерживает OR REPLACE → DELETE+INSERT.
5. ✅ Живой тест: русские запросы «живые цветы»/«цветы» находят картинки
   (раньше — 0-3 случайных). Backfill 27 тыс. картинок идёт в фоне
   (clip_out.log), по завершении весь архив будет искать по содержанию.
6. ✅ Тест +1 (папка в чанке картинки) — **итого 88, все зелёные**.
7. ✅ requirements.txt: +sentence-transformers (torch, transformers — зависимости).
8. ⚠️ Первый прогон backfill упал: vec0 не поддерживает INSERT OR REPLACE
   и «database is locked» при конкуренции с watcher'ом — исправлено
   (DELETE+INSERT, busy_timeout 30 с, ретраи).

### Выполнено дополнительно (инсталлятор: tools.tool_search):
1. ✅ Диагностика: Hermes прячет 27 из 49 инструментов (включая все MCP) за
   discovery-протоколом tool_search/tool_describe/tool_call (~8k токенов промпта);
   облачная Qwen3.7-plus проходит его сама, локальная Qwen3.5-9B — нет
   (ошибки формата в логах, начинает ripgrep/terminal до явного «используй tools»).
2. ✅ Исправление применено вживую: tools.tool_search.enabled: "off" в config.yaml
   Hermes — все 49 инструментов всегда в промпте, discovery-барьер убран.
3. ✅ install_hermes.ps1: шаг 3 — настройка tools.tool_search (три ветки: блок
   есть → обновить; tools: есть → вставить внутрь; нет → добавить в конец),
   YAML-валидация, идемпотентно.
4. ✅ Протестированы 4 сценария (реальный Hermes идемпотентно, свежий Hermes
   без tools:, tools: без tool_search, tool_search: "on" → "off").
5. ✅ README: ручной шаг 3 добавлен в раздел «Интеграция с Hermes».

### НЕ выполнено (следующий шаг):
1. Заполнить `exclude_paths` (UI → «Параметры обработки» → «Исключённые
   пути») — кандидаты: `D:\Backup\Downloads\opencv`, `D:\Backup\Downloads\cmake-4.3.1`,
   возможно `D:\Backup\Downloads` целиком (решает пользователь); после
   сохранения перезапустить watcher/индексацию кнопками.
2. Релиз v0.2.2 по releasing.md (фича + оптимизация → MINOR или PATCH по вкусу).
3. Ручная проверка «спроси в Hermes → ответ со ссылками».

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
- _extract_file: пропуск ~$-lock-файлов Office, try/except вокруг извлечения
  (битый файл → статус error + finish_file, прогон не падает)
- run_index: обход, пауза/стоп-файлы, heartbeat (_hb), пре-подсчёт total
  в daemon-потоке (ETA), prune с защитой от массового удаления;
  конвейер pending/_flush_pending удалён
- path_excluded, content_hash, iter_files, reindex_path, _prune_deleted — ок

### hds/progress.py — РАБОТАЕТ
- RLock (вложенные rate_window/eta_sec безопасны), _seen_ts/total в __init__
- _write(): кодировочно-устойчивая печать (cp1251/pipe не падают)
- _fmt_eta, rate_window(300), eta_sec, heartbeat_data (total/eta/remaining)
- _status_line включает ETA

### hds/watcher.py — РАБОТАЕТ
- reconcile_on_start обёрнут в try/except: сбой сверки не убивает наблюдателя
- _acquire_lock/_pid_alive: stale watch.lock (мёртвый PID) подчищается при старте

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
