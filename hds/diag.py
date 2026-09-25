"""Проверка компонентов окружения: общий код для `hds.cli check` и веб-интерфейса.

Каждая проверка — словарь {"id", "status", "title", "msg", "fix"}:
  status: "ok"   — компонент работает;
          "warn" — деградация части функций (картинки/видео/.mpp/чат-ответы);
          "fail" — критично (индексация и поиск не работают).
Зонды сети короткие (таймаут 3 с, без ретраев), чтобы UI не подвисал.
"""
import os
import shutil
import sys

IS_MAC = sys.platform == "darwin"


def _norm_url(u):
    return (u or "").rstrip("/")


def run_checks(cfg=None):
    from .config import db_abs_path, dig

    if cfg is None:
        from .config import load
        cfg = load()
    checks = []

    def add(cid, status, title, msg="", fix=""):
        checks.append({"id": cid, "status": status, "title": title,
                       "msg": msg, "fix": fix})

    # 1. База данных / sqlite-vec
    db_path = db_abs_path(cfg)
    try:
        from . import db as dbmod
        conn = dbmod.connect(db_path, int(dig(cfg, "embedding.dim", 1024)))
        conn.close()
        add("db", "ok", "База данных: %s" % db_path)
    except OSError as e:
        add("db", "fail", "База данных недоступна: %s" % e,
            "db_path: %s" % db_path,
            "Диск/путь из config.yaml не существует. Исправьте db_path в группе "
            "«Настройки» или перенесите базу (группа «Расположение базы индексации»).")
    except Exception as e:  # noqa: BLE001
        # БД, занятая параллельной записью watcher'а, — это рабочее состояние,
        # а не проблема окружения; настоящие проблемы пути ловятся OSError выше.
        locked = "locked" in str(e).lower() and os.path.exists(db_path)
        add("db", "warn" if locked else "fail",
            "База данных: %s" % e if not locked else
            "База данных занята другим процессом (вероятно, идёт индексация)",
            fix="" if locked else "Проверьте db_path в настройках.")

    # 2. Корни индексации
    roots = [str(r) for r in (dig(cfg, "index.roots", []) or [])]
    if not roots:
        add("roots", "warn", "Корни индексации не заданы",
            fix="Добавьте диски/папки в config.yaml (index.roots) в группе «Настройки».")
    else:
        missing = [r for r in roots if not os.path.isdir(r)]
        if missing:
            add("roots", "warn", "Корни индексации не существуют: %s" % ", ".join(missing),
                fix="Поправьте index.roots в группе «Настройки» — укажите существующий диск/папку.")
        else:
            add("roots", "ok", "Корни индексации: %s" % ", ".join(roots))

    # 3. LLM-серверы llama.cpp — чат-роль (ask_my_files, не критично для поиска)
    from . import llama_server as _ls

    det_chat = _ls.probe(cfg, "chat")
    if det_chat["state"] == _ls.STATE_LLAMA:
        add("chat", "ok", "Чат-сервер llama.cpp отвечает ('%s')"
            % str(dig(cfg, "chat.model", "qwen3.5-9b") or "?"))
    elif det_chat["state"] == _ls.STATE_FOREIGN:
        add("chat", "warn",
            "Порт %s занят посторонним сервисом (не llama-server чат-роли)"
            % _ls.base_url(cfg, "chat"),
            fix="Освободите порт или смените llm_server.chat.port в config.yaml.")
    else:
        add("chat", "warn", "Чат-сервер llama.cpp не запущен — ask_my_files "
            "работает без LLM-ответа (только список найденных файлов)",
            fix="Запустите: python -m hds.llama_server start chat "
                "(или кнопка «Запустить» в группе «LLM-серверы» веб-интерфейса).")

    # 4. Эмбеддинги (роль embedding llama-server) — критично для поиска
    emb_model = dig(cfg, "embedding.model", "text-embedding-bge-m3")
    det_emb = _ls.probe(cfg, "embedding")
    if det_emb["state"] == _ls.STATE_LLAMA:
        add("emb", "ok", "Эмбеддинги: llama-server отвечает ('%s')" % emb_model)
    elif det_emb["state"] == _ls.STATE_FOREIGN:
        add("emb", "fail",
            "Порт %s занят посторонним сервисом (не llama-server роли embedding)"
            % _ls.base_url(cfg, "embedding"),
            fix="Освободите порт или смените llm_server.embedding.port в config.yaml.")
    else:
        emb_model_path = _ls.status(cfg, "embedding")["model"]
        if not os.path.isfile(emb_model_path):
            add("emb", "fail",
                "GGUF-модель эмбеддингов не найдена: %s" % emb_model_path,
                fix="Скачайте кнопкой «Скачать модель» в группе «LLM-серверы» "
                    "веб-интерфейса или запустите установщик общего рантайма: "
                    "installers/ensure_llama_runtime.ps1 -Models embedding "
                    "(macOS: bash installers/ensure_llama_runtime.sh --models embedding). "
                    "Без неё поиск работает только по ключевым словам.")
        else:
            add("emb", "fail", "Эмбеддинг-сервер llama.cpp не запущен",
                fix="Запустите: python -m hds.llama_server start embedding "
                    "(или кнопка «Запустить» в группе «LLM-серверы»). Без неё "
                    "поиск работает только по ключевым словам.")

    # 4b. Контекст embedding-инстанса: меньше EMB_CONTEXT — хвост длинных
    # чанков не попадает в векторы (llama-server вернёт явный 400 по входу
    # длиннее контекста, но лучше перезапустить с нужным контекстом).
    from .config import EMB_CONTEXT
    _ctx = _ls.props_context(det_emb["props"]) \
        if det_emb["state"] == _ls.STATE_LLAMA else None
    if _ctx and _ctx < EMB_CONTEXT:
        add("embctx", "warn",
            "Эмбеддинг-инстанс запущен с контекстом %d (нужно %d) — длинные "
            "фрагменты индексируются неполно" % (_ctx, EMB_CONTEXT),
            fix="Контекст задаёт llm_server.embedding.ctx_per_slot (%d): "
                "python -m hds.llama_server restart embedding, затем "
                "переиндексация («Старт (переобработка всего)»)." % EMB_CONTEXT)

    # 5. Tesseract OCR
    from .extractors import _tesseract_ready
    if _tesseract_ready(cfg):
        add("ocr", "ok", "Tesseract OCR найден")
    else:
        add("ocr", "warn", "Tesseract OCR не найден — картинки и сканы без текстового слоя не индексируются",
            fix=("Установите: brew install tesseract tesseract-lang" if IS_MAC else
                 "Установите: winget install UB-Mannheim.TesseractOCR (пакет русского языка "
                 "'rus' — github.com/tesseract-ocr/tessdata); путь к tesseract.exe — "
                 "index.ocr_tesseract_cmd в настройках."))

    # 6. ffmpeg
    if shutil.which("ffmpeg"):
        add("ffmpeg", "ok", "ffmpeg найден")
    else:
        add("ffmpeg", "warn", "ffmpeg не найден — видео без транскрипции",
            fix=("Установите: brew install ffmpeg" if IS_MAC else
                 "Установите: winget install Gyan.FFmpeg (после установки откройте "
                 "новое окно и перезапустите watcher/индексацию)."))

    # 7. faster-whisper
    try:
        import faster_whisper  # noqa: F401
        dev_desc = ""
        try:
            from .extract_av import _pick_device
            dev, comp = _pick_device(cfg)
            dev_name = {"metal-mlx": "Metal (mlx-whisper)",
                        "vulkan": "Vulkan (whisper.cpp)"}.get(dev, dev)
            dev_desc = " (транскрипция: %s/%s)" % (dev_name, comp)
        except Exception:  # noqa: BLE001
            pass
        add("whisper", "ok", "faster-whisper установлен" + dev_desc)
    except Exception:  # noqa: BLE001
        py = ".venv/bin/python" if IS_MAC else ".venv\\Scripts\\python.exe"
        add("whisper", "warn", "faster-whisper не установлен — аудио/видео без транскрипции",
            fix="Установите: %s -m pip install faster-whisper" % py)

    # 7b. GPU без CUDA (AMD/Intel на Windows): whisper.cpp Vulkan
    if os.name == "nt":
        try:
            import ctranslate2 as _ct
            cuda_n = _ct.get_cuda_device_count()
        except Exception:  # noqa: BLE001
            cuda_n = 0
        if cuda_n == 0:
            from . import whisper_cpp
            gpu_ok, gpu_names = whisper_cpp.gpu_vendor_present()
            if gpu_ok and not whisper_cpp.available(cfg):
                add("vulkan", "warn",
                    "GPU без CUDA обнаружена (%s), но Vulkan-транскрипция (whisper.cpp) "
                    "не установлена — аудио/видео будут обрабатываться медленно на CPU"
                    % (gpu_names or "GPU"),
                    fix="Запустите setup.ps1 повторно (поставит whisper.cpp Vulkan "
                        "автоматически) или выполните: .venv\\Scripts\\python.exe -m hds.cli vulkan-setup")

    # 7c. pymorphy3 — русская морфология ключевого поиска (FTS)
    from . import lemmatizer
    if lemmatizer.available():
        add("lemmatizer", "ok", "pymorphy3 установлен — русская морфология в ключевом поиске")
        try:
            c = dbmod.connect(db_path, int(dig(cfg, "embedding.dim", 1024)))
            total = c.execute("SELECT COUNT(*) FROM chunks").fetchone()[0]
            stale = 0
            if total:
                # Содержательная проверка вместо флага meta 'fts_normalized':
                # обычная индексация всегда пишет лемматизированный FTS
                # (db.add_chunk -> lemmatizer.normalize), но флаг ставила только
                # команда reindex-fts — на свежей установке (индексация с нуля)
                # предупреждение срабатывало ложно. Сэмплируем чанки с начала
                # таблицы (там и лежат самые старые данные) и сверяем FTS-текст
                # с нормализацией: совпадает — лемматизация есть.
                stride = max(1, total // 100)
                rows = c.execute(
                    "SELECT c.text, f.text FROM chunks c "
                    "JOIN chunks_fts f ON f.rowid = c.id "
                    "WHERE c.id % ? = 0 LIMIT 100", (stride,)).fetchall()
                for ch, ft in rows:
                    if ft != lemmatizer.normalize(ch):
                        stale += 1
            c.close()
            if stale:
                add("fts-norm", "warn",
                    "Часть FTS-чанков (%d из %d сэмпла) проиндексирована без "
                    "лемматизации — разные словоформы не находятся на данных, "
                    "проиндексированных раньше" % (stale, min(100, total)),
                    fix="Запустите: python -m hds.cli reindex-fts (только CPU, "
                        "30-90 мин на ~550 тыс. чанков, без переэмбеддинга)")
        except Exception:  # noqa: BLE001
            pass
    else:
        add("lemmatizer", "warn",
            "pymorphy3 не установлен — ключевой поиск без русской морфологии "
            "(«настройки» не находит «настройка»)",
            fix="Установите: pip install pymorphy3 pymorphy3-dicts-ru, затем "
                "python -m hds.cli reindex-fts")

    # 7d. реранкер (только если включён в конфиге)
    if dig(cfg, "rerank.enabled", False):
        import requests as _rq

        base = (dig(cfg, "rerank.url", "http://localhost:8012/v1") or "").rstrip("/")
        try:
            ok = _rq.get(base + "/health", timeout=3).status_code == 200
        except Exception:  # noqa: BLE001
            ok = False
        if ok:
            add("rerank", "ok", "Реранкер (llama-server) отвечает")
        else:
            add("rerank", "warn",
                "rerank.enabled включён, но реранкер не отвечает — "
                "ask_my_files работает без реранкинга",
                fix="Запустите роль реранкера менеджером: python -m hds.llama_server "
                    "start rerank (модель llm_server.rerank.model, GGUF ~600 МБ; "
                    "LM Studio /rerank не реализует).")

    # 8. mpxj (MS Project)
    try:
        import mpxj  # noqa: F401
        add("mpp", "ok", "mpxj (MS Project) установлен")
    except Exception:  # noqa: BLE001
        add("mpp", "warn", "mpxj не установлен — файлы .mpp не индексируются",
            fix=("Установите: .venv/bin/python -m pip install mpxj (нужна Java 11+)." if IS_MAC else
                 "Установите: pip install mpxj (нужна Java 11+: системная JDK или "
                 "%LOCALAPPDATA%\\jdk-21\\)."))

    return checks


def has_failures(checks):
    """True, если есть критичные проблемы (индексация/поиск не заработают)."""
    return any(c["status"] == "fail" for c in checks)
