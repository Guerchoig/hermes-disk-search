"""Загрузка конфигурации config.yaml (кодировка UTF-8, BOM допускается)."""
import os

import yaml

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def config_path():
    return os.environ.get("HDS_CONFIG") or os.path.join(PROJECT_ROOT, "config.yaml")


def _default_config_yaml():
    home = os.path.expanduser("~")
    return (
        "# hermes-disk-search — конфигурация (создана автоматически)\n"
        "# Корни индексации (диски/папки). Добавляйте по одному на строку.\n"
        "# Редактировать можно из веб-интерфейса (run_ui.ps1 / «Hermes Disk Search»).\n"
        "index:\n"
        "  roots:\n"
        "    - '%s'\n" % home.replace("'", "''") +
        "  exclude_dirs: [\"$RECYCLE.BIN\", \"System Volume Information\", \"node_modules\","
        " \".git\", \".venv\", \"venv\", \"__pycache__\", \"AppData\", \"Windows\","
        " \"Windows.old\", \"Recovery\", \".cache\", \".npm\", \".nuget\", \"packages\","
        " \"hermes-disk-search-db\", \"WindowsApps\", \".Trash\", \".Trashes\", \"models\"]\n"
        "  max_file_mb: 200          # предел для обычных файлов\n"
        "  max_media_mb: 2500        # предел для аудио/видео (транскрипция)\n"
        "  ocr: true                 # OCR картинок и пустых страниц PDF (нужен Tesseract)\n"
        "  ocr_lang: \"rus+eng\"\n"
        "  transcribe: true          # транскрипция аудио/видео (нужен faster-whisper)\n"
        "  whisper_model: small\n"
        "\n"
        "chunk:\n"
        "  size: 1200               # символов в чанке\n"
        "  overlap: 200             # перекрытие чанков\n"
        "\n"
        "embedding:\n"
        "  base_url: \"http://localhost:1234/v1\"   # LM Studio\n"
        "  model: \"text-embedding-bge-m3\"\n"
        "  batch_size: 64\n"
        "  dim: 1024                # bge-m3 = 1024\n"
        "\n"
        "chat:\n"
        "  base_url: \"http://localhost:1234/v1\"\n"
        "  model: \"qwen3.5-9b\"\n"
        "  temperature: 0.2\n"
        "\n"
        "search:\n"
        "  vec_k: 40\n"
        "  fts_k: 40\n"
        "  rrf_k: 60\n"
        "  snippet_chars: 500\n"
    )


def ensure_config():
    """Создать config.yaml с настройками по умолчанию, если файл отсутствует.

    Возвращает True, если файл был создан. Гарантирует, что UI и CLI
    стартуют «из коробки» без ручного редактирования настроек.
    """
    p = config_path()
    if os.path.exists(p):
        return False
    os.makedirs(os.path.dirname(p) or ".", exist_ok=True)
    with open(p, "w", encoding="utf-8") as f:
        f.write(_default_config_yaml())
    return True


def load(path=None):
    p = path or config_path()
    with open(p, "r", encoding="utf-8-sig") as f:
        return yaml.safe_load(f) or {}


def dig(cfg, dotted, default=None):
    """Достать значение по пути вида 'index.roots'."""
    cur = cfg
    for part in dotted.split("."):
        if not isinstance(cur, dict) or part not in cur:
            return default
        cur = cur[part]
    return cur


def db_abs_path(cfg):
    p = dig(cfg, "db_path", "index.db")
    if not os.path.isabs(p):
        p = os.path.join(PROJECT_ROOT, p)
    return p