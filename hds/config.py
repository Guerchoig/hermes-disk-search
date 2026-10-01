"""Загрузка конфигурации config.yaml (кодировка UTF-8, BOM допускается)."""
import os

import yaml

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# Имя приложения в MCP (serverInfo.name) и маркер health-эндпоинта
# (/health: {"app": APP_NAME}): по нему менеджер hds.mcp_http отличает СВОЙ
# живой http-инстанс от чужого сервиса, занявшего тот же порт (probe).
APP_NAME = "disk-search"

# Целевой контекст embedding-модели (bge-m3) в llama-server (роль embedding).
# При меньшем загруженном контексте вход отсекается: llama.cpp отвечает явной
# ошибкой 400, LM Studio (старый бэкенд) усекал МОЛЧА (проверено: текст 15 644
# токена при ctx=8192 дал вектор, равный вектору первых ~8192 токенов, без
# ошибки в ответе). Чанки проекта: медиана 484 токена, p90 829, максимум 2114 —
# при ctx=512 ~45 % чанков теряли бы хвост.
EMB_CONTEXT = 8192


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
        "  ocr_tesseract_cmd: \"\"   # путь к tesseract(.exe), если не в PATH (задаёт установщик)\n"
        "  transcribe: true          # транскрипция аудио/видео (нужен faster-whisper)\n"
        "  whisper_model: small\n"
        "  whisper_device: auto      # auto | cuda | vulkan | cpu | metal\n"
        "  whisper_load_timeout: 180 # сек; зависание CUDA-загрузки — fallback на CPU\n"
        "  whisper_batch: 8          # батчевая транскрипция (0 = отключить)\n"
        "  whisper_language: \"\"      # \"\" = авто-детекция; \"ru\" — фиксировать русский\n"
        "  whisper_mode: subtitle    # W3: subtitle (таймкоды) | speech (текст)\n"
        "  whisper_custom: \"4.5\"     # W3: окно субтитров, сек\n"
        "  whisper_gpu: 0            # W3: индекс GPU движка (-1 = CPU)\n"
        "  transcribe_url: \"http://127.0.0.1:8010\"  # W3: владелец GPU /internal/transcribe\n"
        "  clip: true                # CLIP-поиск картинок по содержанию\n"
        "  max_chunks: 3000          # максимум чанков на файл (0 = без лимита)\n"
        "\n"
        "chunk:\n"
        "  size: 800                # символов в чанке (≈400 токенов; точечные запросы)\n"
        "  overlap: 120             # перекрытие соседних чанков (целые предложения)\n"
        "\n"
        "embedding:\n"
        "  base_url: \"http://127.0.0.1:8011/v1\"  # llama-server (hds.llama_server)\n"
        "  model: \"text-embedding-bge-m3\"\n"
        "  batch_size: 64\n"
        "  dim: 1024                # bge-m3 = 1024\n"
        "\n"
        "chat:\n"
        "  base_url: \"http://127.0.0.1:8010/v1\"\n"
        "  model: \"qwen3.5-9b\"      # --alias llama-server чат-роли\n"
        "  thinking: \"off\"          # off|auto: off — RAG-ответы без размышлений (быстро)\n"
        "  temperature: 0.2\n"
        "\n"
        "llm_server:\n"
        "  bin: \"\"                    # путь к llama-server; пусто: общий llama-рантайм > tools/llama.cpp/ > PATH\n"
        "  host: \"127.0.0.1\"\n"
        "  autostart: true            # поднимать серверы при старте UI/MCP/cli\n"
        "  start_timeout: 300         # сек ожидания /health при старте\n"
        "  parallel: 1                # 1 запрос одновременно, остальные в очереди\n"
        "  chat:\n"
        "    port: 8010\n"
        "    model: \"shared:chat\"      # shared:<role> — GGUF общего llama-рантайма\n"
        "    ctx_per_slot: 32768      # 32K: нужно и общему инстансу с прокси-проектом\n"
        "    extra_args: \"--cache-type-k q8_0 --cache-type-v q8_0 -ngl 99\"\n"
        "  embedding:\n"
        "    port: 8011\n"
        "    model: \"shared:embedding\"\n"
        "    ctx_per_slot: 8192\n"
        "    extra_args: \"--batch-size 8192 --ubatch-size 8192 -ngl 99\"\n"
        "  rerank:\n"
        "    port: 8012\n"
        "    model: \"shared:rerank\"\n"
        "    ctx_per_slot: 8192\n"
        "    # --batch-size/--ubatch-size обязательны: фрагмент длиннее physical\n"
        "    # batch (дефолт 512) даёт 500 (\"input is too large to process\")\n"
        "    extra_args: \"-ngl 0 --batch-size 8192 --ubatch-size 8192\"\n"
        "\n"
        "mcp_http:\n"
        "  host: \"127.0.0.1\"\n"
        "  port: 8787               # ОДИН http-инстанс MCP на машину (клиенты идут по URL)\n"
        "  path: \"/mcp\"             # streamable-http endpoint\n"
        "  autostart: true          # поднимать при старте UI/MCP/cli (переиспользует живой)\n"
        "  start_timeout: 30        # сек ожидания /health при старте\n"
        "\n"
        "search:\n"
        "  vec_k: 40\n"
        "  fts_k: 40\n"
        "  rrf_k: 60\n"
        "  fts_weight: 1.0          # вес FTS-ветки в RRF-слиянии\n"
        "  vec_weight: 1.0          # вес векторной ветки\n"
        "  snippet_chars: 500\n"
        "\n"
        "rerank:\n"
        "  enabled: false           # реранкер для ask_my_files (bge-reranker-v2-m3 в llama-server)\n"
        "  url: \"http://localhost:8012/v1\"\n"
        "  model: \"bge-reranker-v2-m3\"\n"
        "  timeout: 30\n"
        "  max_latency: 15          # сек; превышение — реранкер авто-отключается\n"
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


def replace_file(src, dst, attempts=20, delay=0.05):
    """Атомарная замена файла с ретраями.

    os.replace на Windows падает (PermissionError), если целевой файл в момент
    замены открыт другим потоком/процессом хотя бы на чтение (CRT не передаёт
    FILE_SHARE_DELETE). UI-сервер читает config.yaml каждые 2 секунды из
    соседних потоков, поэтому конкурентные записи в него требуют ретраев.
    """
    import time

    for i in range(attempts):
        try:
            os.replace(src, dst)
            return
        except PermissionError:
            if i == attempts - 1:
                raise
            time.sleep(delay)


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