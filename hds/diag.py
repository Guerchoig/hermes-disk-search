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

    # 3. LM Studio + чат-модель
    import requests as rq
    base = _norm_url(dig(cfg, "chat.base_url", "http://localhost:1234/v1"))
    try:
        r = rq.get(base + "/models", timeout=3)
        models = [m.get("id") for m in r.json().get("data", [])]
        chat_model = dig(cfg, "chat.model", "")
        if chat_model and models and chat_model not in models:
            # LM Studio может отдавать id без суффикса квантизации ('qwen3.5-9b@q6_k').
            b = chat_model.split("@")[0].strip().lower()
            near = next((m for m in models if m and m.split("@")[0].strip().lower() == b), None)
            if near:
                add("chat", "warn",
                    "Чат-модель '%s' не совпадает с загруженной '%s'" % (chat_model, near),
                    fix="Уточните имя chat.model в группе «Настройки».")
            else:
                add("chat", "warn", "Чат-модель '%s' не загружена" % chat_model,
                    "доступно: %s" % ", ".join(m for m in models if m)[:160],
                    "В LM Studio: Developer → Select a model to load → %s "
                    "(если модели нет — скачайте её в LM Studio, раздел Search)." % chat_model)
        else:
            add("chat", "ok", "Чат-модель '%s' отвечает" % (chat_model or "?"))
    except Exception as e:  # noqa: BLE001
        add("chat", "fail", "LM Studio недоступен (%s)" % base, str(e),
            "Установите LM Studio (https://lmstudio.ai), запустите сервер "
            "(Developer → Start Server) и загрузите чат-модель.")

    # 4. Эмбеддинги (короткий зонд без ретраев)
    emb_model = dig(cfg, "embedding.model", "text-embedding-bge-m3")
    emb_url = _norm_url(dig(cfg, "embedding.base_url", "http://localhost:1234/v1"))
    try:
        r = rq.post(emb_url + "/embeddings",
                    json={"model": emb_model, "input": ["ping"]}, timeout=3)
        if r.status_code == 200:
            add("emb", "ok", "Эмбеддинги: модель '%s' отвечает" % emb_model)
        else:
            raise RuntimeError("HTTP %s" % r.status_code)
    except Exception:  # noqa: BLE001
        # сервер может знать модель под другим идентификатором (bge-m3) —
        # ищем её в списке моделей и пробуем зонд с фактическим именем
        actual = None
        try:
            r2 = rq.get(emb_url + "/models", timeout=3)
            for m in (m.get("id") or "" for m in r2.json().get("data", [])):
                if "bge-m3" in m.lower():
                    actual = m
                    break
        except Exception:  # noqa: BLE001
            pass
        handled = False
        if actual and actual != emb_model:
            try:
                r3 = rq.post(emb_url + "/embeddings",
                             json={"model": actual, "input": ["ping"]}, timeout=3)
                if r3.status_code == 200:
                    add("emb", "warn",
                        "Эмбеддинги работают, но сервер отдаёт модель под именем '%s', "
                        "а в config.yaml указано '%s'" % (actual, emb_model),
                        fix="В группе «Модель эмбеддингов» нажмите «Применить имя "
                            "модели» — embedding.model в настройках обновится "
                            "автоматически.")
                    handled = True
            except Exception:  # noqa: BLE001
                pass
        if not handled:
            add("emb", "fail",
                "Эмбеддинги недоступны: модель '%s' не загружена" % emb_model,
                fix="Скачайте модель и загрузите её в LM Studio (тип Embedding): "
                    "кнопка «Скачать модель» в группе «Модель эмбеддингов» выше, затем "
                    "«Загрузить в LM Studio». Без неё поиск работает только по ключевым словам.")

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
