"""Менеджер llama-server (llama.cpp) — локальный LLM-бэкенд проекта.

Одна GGUF-модель = один llama-server (llama.cpp поднимает OpenAI-совместимый
HTTP-сервер на одном процессе). Проекту нужны три модели с разными режимами
инференса, поэтому менеджер управляет ТРЕМЯ инстансами (роли):

  chat      порт 8010 — чат-генерация (RAG в hds/rag.py), БЕЗ tool-calling
            (инструменты вызываются агентом, а не моделью);
  embedding порт 8011 — bge-m3, `--embedding --pooling cls` (пулинг сверен
            замером PLAN_INDEX_QUALITY.md A3: cos >= 0.9987 с LM Studio);
  rerank    порт 8012 — bge-reranker-v2-m3, `--reranking --pooling rank`.

Каждая роль: свой PID-файл (data/llama_<role>.pid) и лог
(data/logs/llama_<role>.log). Запуск отвязанный (переживает перезапуск
UI/MCP/CLI). Все инстансы стартуют с --parallel 1: одновременно работает
ровно один запрос, остальные ждут в очереди llama-server.

Конфигурация — секция llm_server в config.yaml (defaults в коде):

    llm_server:
      bin: ""            # путь к llama-server; пусто: общий llama-рантайм > tools/llama.cpp/ > PATH
      host: "127.0.0.1"
      autostart: true
      start_timeout: 300
      parallel: 1        # --ctx-size = parallel * ctx_per_slot
      chat/embedding/rerank: {port, model, ctx_per_slot, extra_args}

Модель каждой роли ищется относительно корня проекта; имя модели в API
(--alias) берётся из config (chat.model / embedding.model / rerank.model),
чтобы клиенты не зависели от имени GGUF-файла.

CLI:
    python -m hds.llama_server check [role]    # exit 0 — живой инстанс
    python -m hds.llama_server start [role|all]
    python -m hds.llama_server stop [role|all]
    python -m hds.llama_server status [role|all]
    python -m hds.llama_server restart [role|all]
    python -m hds.llama_server run <role>      # foreground (автозапуск ОС)

Только стандартная библиотека (urllib/subprocess/threading).
"""
import json
import os
import shutil
import subprocess
import sys
import threading
import time
import urllib.request

from .config import PROJECT_ROOT, dig

ROLES = ("chat", "embedding", "rerank")

STATE_LLAMA = "llama"      # живой llama-server с ОЖИДАЕМОЙ моделью
STATE_FOREIGN = "foreign"  # порт занят чужим сервисом (в т.ч. чужой llama)
STATE_DOWN = "down"        # никто не слушает

_PROBE_TIMEOUT = 3.0
_POLL_INTERVAL = 2.0

# Дефолты ролей (перекрываются секцией llm_server.<role> в config.yaml)
_DEFAULTS = {
    "chat": {
        "port": 8010,
        "model": "models/chat/qwen3.5-9b-Q6_K.gguf",
        # 32K: столько требует прокси-проект (anonymizer_proxy) для длинных
        # файлов, когда он ПЕРЕИСПОЛЬЗУЕТ этот инстанс (общий llama-сервер).
        # Одному HDS хватало 16K, но один инстанс на две программы экономичнее.
        "ctx_per_slot": 32768,
        # KV-кэш q8_0: вдвое меньше VRAM (12 ГБ-машина: chat Q6_K + emb на GPU)
        "extra_args": "--cache-type-k q8_0 --cache-type-v q8_0 -ngl 99",
    },
    "embedding": {
        "port": 8011,
        "model": "models/embedding/bge-m3-Q8_0.gguf",
        # 8192 — жёсткое требование (hds/config.py: EMB_CONTEXT): при меньшем
        # контексте длинные чанки теряют хвост (llama-server вернёт явный 400)
        "ctx_per_slot": 8192,
        "extra_args": "--batch-size 8192 --ubatch-size 8192 -ngl 99",
    },
    "rerank": {
        "port": 8012,
        "model": "models/rerank/bge-reranker-v2-m3-Q8_0.gguf",
        "ctx_per_slot": 8192,
        "extra_args": "-ngl 0",  # реранкер по умолчанию на CPU
    },
}

# Режимные флаги роли (единое место; проверяется тестами).
_MODE_FLAGS = {
    "chat": [],  # БЕЗ --jinja: tool-calling остаётся на стороне агента (дэдлоки)
    "embedding": ["--embedding", "--pooling", "cls"],
    "rerank": ["--reranking", "--pooling", "rank"],
}
_MODEL_KEY = {"chat": "chat.model", "embedding": "embedding.model",
              "rerank": "rerank.model"}


def _role_cfg(cfg, role):
    """Секция llm_server.<role>, слитая с дефолтами."""
    sec = (dig(cfg, "llm_server") or {}).get(role) or {}
    d = _DEFAULTS[role]
    return {
        "port": int(sec.get("port", d["port"])),
        "model": sec.get("model", d["model"]),
        "ctx_per_slot": int(sec.get("ctx_per_slot", d["ctx_per_slot"])),
        "extra_args": (sec.get("extra_args", d["extra_args"]) or "").strip(),
    }


def host(cfg):
    return str((cfg.get("llm_server") or {}).get("host", "127.0.0.1"))


def parallel(cfg):
    return max(1, int((cfg.get("llm_server") or {}).get("parallel", 1)))


def start_timeout(cfg):
    return float((cfg.get("llm_server") or {}).get("start_timeout", 300))


def autostart_on(cfg):
    return bool((cfg.get("llm_server") or {}).get("autostart", True))


def base_url(cfg, role):
    r = _role_cfg(cfg, role)
    return "http://%s:%d" % (host(cfg), r["port"])


def api_url(cfg, role):
    return base_url(cfg, role) + "/v1"


def _abs_model(cfg, role):
    """Путь GGUF роли.

    Спецификатор shared:<role> — модель из манифеста ОБЩЕГО llama-рантайма
    машины (llama_runtime.py: тот же файл, что использует anonymizer_proxy),
    что даёт синхронную смену чат-модели во всех проектах. Иначе — обычный
    путь (относительный разрешается от корня проекта).
    """
    from . import llama_runtime

    p = str(_role_cfg(cfg, role)["model"] or "").strip()
    if p.startswith(llama_runtime.SHARED_PREFIX):
        try:
            return os.path.normpath(str(llama_runtime.resolve_model(p, role)))
        except FileNotFoundError:
            # рантайм/модель ещё не установлены — отдаём каталог роли:
            # build_command покажет понятную ошибку с путём
            return os.path.normpath(str(llama_runtime.models_dir(role)))
    if not os.path.isabs(p) and len(p) > 2 and p[1] != ":":
        p = os.path.join(PROJECT_ROOT, p)
    # Windows-путь с буквой диска os.path.isabs распознаёт и так
    return os.path.normpath(p)


def _pid_file(role):
    return os.path.join(PROJECT_ROOT, "data", "llama_%s.pid" % role)


def _log_file(role):
    return os.path.join(PROJECT_ROOT, "data", "logs", "llama_%s.log" % role)


# ==================== HTTP-пробы ====================

def _get_json(url, timeout=_PROBE_TIMEOUT):
    """GET с таймаутом; dict при 2xx-JSON, иначе исключение."""
    req = urllib.request.Request(url, headers={"Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        if not 200 <= resp.status < 300:
            raise OSError("HTTP %s" % resp.status)
        return json.loads(resp.read().decode("utf-8", errors="replace"))


def probe(cfg, role, timeout=_PROBE_TIMEOUT):
    """Определить, кто слушает порт роли.

    Живой llama распознаётся по GET /props с целым total_slots >= 1.
    Дополнительно сверяем model_path из /props с ожидаемым GGUF роли:
    чужой llama с другой моделью = STATE_FOREIGN (не переиспользуем).
    Возвращает {"state": STATE_*, "total_slots": int|None, "props": dict}.
    """
    url = base_url(cfg, role)
    try:
        with urllib.request.urlopen(url + "/health", timeout=timeout) as resp:
            if not 200 <= resp.status < 300:
                raise OSError("HTTP %s" % resp.status)
    except Exception:  # noqa: BLE001
        return {"state": STATE_DOWN, "total_slots": None, "props": {}}
    try:
        props = _get_json(url + "/props", timeout)
        slots = props.get("total_slots")
        if not (isinstance(slots, int) and slots >= 1):
            return {"state": STATE_FOREIGN, "total_slots": None, "props": {}}
        actual = props.get("model_path") or props.get("model") or ""
        expected = _abs_model(cfg, role)
        if actual and os.path.normcase(os.path.normpath(actual)) != \
                os.path.normcase(expected):
            return {"state": STATE_FOREIGN, "total_slots": slots, "props": props}
        return {"state": STATE_LLAMA, "total_slots": slots, "props": props}
    except Exception:  # noqa: BLE001
        return {"state": STATE_FOREIGN, "total_slots": None, "props": {}}


def props_context(props):
    """Фактический контекст инстанса из /props (None, если неизвестен)."""
    d = props.get("default_generation_settings") or {}
    ctx = d.get("n_ctx") or props.get("n_ctx")
    try:
        return int(ctx) if ctx else None
    except (TypeError, ValueError):
        return None


# ==================== Бинарь и команда запуска ====================

def find_binary(cfg):
    """Путь к llama-server: llm_server.bin > общий рантайм > tools/llama.cpp/ > PATH.

    Общий llama-рантайм машины (llama_runtime.py) — первое звено после
    явного llm_server.bin: одна сборка llama.cpp на все проекты
    (Windows — пре-билд в %LOCALAPPDATA%\\llama-runtime\\bin,
    macOS — bin/ рантайма со ссылкой на бинарь Homebrew).
    '' — не найден.
    """
    b = str((cfg.get("llm_server") or {}).get("bin") or "").strip()
    if b:
        return b
    from . import llama_runtime

    shared = llama_runtime.find_binary()
    if shared:
        return shared
    exe = "llama-server.exe" if sys.platform == "win32" else "llama-server"
    local = os.path.join(PROJECT_ROOT, "tools", "llama.cpp", exe)
    if os.path.isfile(local):
        return local
    found = shutil.which("llama-server")
    return str(found) if found else ""


def _ensure_hint(role=None):
    """Команда установщика общего рантайма для текущей ОС (тексты ошибок).

    Windows — установщик PowerShell, macOS/Linux — bash-скрипт
    (SYNC-COPY-пара: installers/ensure_llama_runtime.ps1 | .sh).
    """
    models = role or "chat,embedding,rerank"
    if sys.platform == "win32":
        return "installers\\ensure_llama_runtime.ps1 -Models %s" % models
    return "bash installers/ensure_llama_runtime.sh --models %s" % models


def build_command(cfg, role):
    """Командная строка llama-server роли.

    --port/--host всегда явно; --ctx-size = parallel * ctx_per_slot.
    --alias — имя модели из config (chat.model / embedding.model /
    rerank.model): клиенты не зависят от имени GGUF-файла.
    """
    r = _role_cfg(cfg, role)
    bin_path = find_binary(cfg)
    if not bin_path:
        raise RuntimeError(
            "llama-server не найден (llm_server.bin пуст, общий llama-рантайм "
            "и tools/llama.cpp/ проекта пусты, PATH пуст). Установите общий "
            "рантайм: %s — либо укажите путь в llm_server.bin"
            % _ensure_hint())
    model = _abs_model(cfg, role)
    if not os.path.isfile(model):
        raise RuntimeError(
            "GGUF-модель роли '%s' не найдена: %s. Скачайте её "
            "(%s или кнопка «Скачать модель» в UI) "
            "или укажите другой путь в llm_server.%s.model"
            % (role, model, _ensure_hint(role), role))
    alias = str(dig(cfg, _MODEL_KEY[role], role) or role)
    par = parallel(cfg)
    total_ctx = par * r["ctx_per_slot"]
    cmd = [
        bin_path,
        "-m", model,
        "--alias", alias,
        "--host", host(cfg),
        "--port", str(r["port"]),
        "--parallel", str(par),
        "--ctx-size", str(total_ctx),
        # WebUI llama-server не нужен: UI проекта — страницы hds.ui_server
        "--no-webui",
        "--cache-reuse", "256",
    ]
    cmd += list(_MODE_FLAGS[role])
    extra = (r["extra_args"] or "").split()
    if extra:
        cmd += extra
    return cmd


# ==================== Запуск / остановка ====================

def _no_window():
    """Флаги subprocess: консольные утилиты (tasklist/taskkill) не должны
    вспыхивать окнами при вызове из UI (Windows)."""
    return {"creationflags": subprocess.CREATE_NO_WINDOW} \
        if sys.platform == "win32" else {}


def _popen_kwargs():
    """Отвязанный запуск: llama-server переживает перезапуск UI/MCP/CLI.

    CREATE_NO_WINDOW в дополнение к DETACHED_PROCESS: старт из UI/MCP
    (pythonw/автозапуск ОС) не должен рождать мигающее консольное окно
    llama-server (Windows).
    """
    if sys.platform == "win32":
        return {"creationflags": (subprocess.DETACHED_PROCESS
                                  | subprocess.CREATE_NEW_PROCESS_GROUP
                                  | subprocess.CREATE_NO_WINDOW)}
    return {"start_new_session": True}


def _read_pid(role):
    try:
        with open(_pid_file(role), "r", encoding="utf-8") as f:
            return int(f.read().strip())
    except (ValueError, OSError):
        return None


def _live_pid(role):
    """PID из PID-файла, если процесс с ним ещё жив."""
    pid = _read_pid(role)
    if pid is None:
        return None
    if sys.platform == "win32":
        try:
            res = subprocess.run(["tasklist", "/FI", "PID eq %d" % pid],
                                 capture_output=True, text=True, check=False,
                                 **_no_window())
            return pid if str(pid) in (res.stdout or "") else None
        except OSError:
            return None
    try:
        os.kill(pid, 0)
        return pid
    except OSError:
        return None


def _wait_ready(cfg, role, proc):
    """Поллинг /health до готовности (загрузка GGUF и KV-кэша — десятки сек)."""
    timeout = start_timeout(cfg)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(
                "llama-server (%s) завершился с кодом %s при старте — см. лог %s"
                % (role, proc.returncode, _log_file(role)))
        det = probe(cfg, role)
        if det["state"] == STATE_LLAMA:
            return det
        time.sleep(_POLL_INTERVAL)
    raise RuntimeError(
        "llama-server (%s) не ответил на /health за %d c — см. лог %s"
        % (role, int(timeout), _log_file(role)))


def start(cfg, role, wait=True):
    """Проверить порт роли и при необходимости запустить llama-server.

    Живой инстанс с ОЖИДАЕМОЙ моделью переиспользуем (в т.ч. запущенный
    вручную). Посторонний сервис (в т.ч. чужой llama с другой моделью) —
    ошибка: молча менять порт нельзя, на этот адрес смотрят клиенты.
    Возвращает {"started", "reused", "state", "total_slots", "pid", "command"}.
    """
    det = probe(cfg, role)
    if det["state"] == STATE_LLAMA:
        return {"started": False, "reused": True, "state": det["state"],
                "total_slots": det["total_slots"], "pid": _read_pid(role),
                "command": []}
    if det["state"] == STATE_FOREIGN:
        raise RuntimeError(
            "Порт %s занят посторонним сервисом (не llama-server с моделью "
            "роли '%s'). Освободите порт или смените llm_server.%s.port в "
            "config.yaml" % (base_url(cfg, role), role, role))

    cmd = build_command(cfg, role)
    log_path = _log_file(role)
    os.makedirs(os.path.dirname(log_path), exist_ok=True)
    with open(log_path, "ab") as log:
        try:
            proc = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                                    stdout=log, stderr=subprocess.STDOUT,
                                    **_popen_kwargs())
        except OSError as exc:
            raise RuntimeError(
                "Не удалось запустить llama-server (%s): %s. Проверьте "
                "llm_server.bin в config.yaml." % (cmd[0], exc)) from exc
    with open(_pid_file(role), "w", encoding="utf-8") as f:
        f.write(str(proc.pid))
    if not wait:
        return {"started": True, "reused": False, "state": STATE_DOWN,
                "total_slots": None, "pid": proc.pid, "command": cmd}
    det = _wait_ready(cfg, role, proc)
    return {"started": True, "reused": False, "state": STATE_LLAMA,
            "total_slots": det["total_slots"], "pid": proc.pid, "command": cmd}


def stop(cfg, role):
    """Остановить инстанс роли по PID-файлу. True — процесс был и убит.

    Инстанс, запущенный вне менеджера (нет PID-файла), не трогаем."""
    pid = _read_pid(role)
    if pid is None:
        return False
    try:
        if sys.platform == "win32":
            subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"],
                           capture_output=True, check=False,
                           **_no_window())
        else:
            os.kill(pid, 15)  # SIGTERM
    except (ProcessLookupError, OSError):
        pass
    try:
        os.remove(_pid_file(role))
    except OSError:
        pass
    return True


def status(cfg, role):
    """Сводный статус инстанса роли (для /api/diagnostics и UI)."""
    det = probe(cfg, role)
    r = _role_cfg(cfg, role)
    model = _abs_model(cfg, role)
    return {
        "role": role,
        "base_url": base_url(cfg, role),
        "state": det["state"],
        "running": det["state"] == STATE_LLAMA,
        "total_slots": det["total_slots"],
        "ctx_configured": parallel(cfg) * r["ctx_per_slot"],
        "ctx_actual": props_context(det["props"]),
        "model": model,
        "model_ready": os.path.isfile(model),
        "alias": str(dig(cfg, _MODEL_KEY[role], role) or role),
        "pid": _live_pid(role),
        "binary": find_binary(cfg),
    }


def ensure(cfg, roles=None):
    """Гарантировать живые инстансы нужных ролей (блокирующе, по очереди).

    Ошибки одной роли не мешают остальным — собираются в результат.
    Возвращает {role: info-словарь или RuntimeError-строка}.
    """
    roles = roles or ("chat", "embedding")
    out = {}
    for role in roles:
        try:
            out[role] = start(cfg, role, wait=True)
        except Exception as e:  # noqa: BLE001 — вызывающий логирует
            out[role] = {"error": str(e)}
    return out


def ensure_async(cfg, roles=None):
    """ensure() в фоновом потоке — для старта UI/MCP/CLI (неблокирующе)."""

    def job():
        try:
            info = ensure(cfg, roles)
            for role, det in info.items():
                if "error" in det:
                    print("[llama_server] %s: %s" % (role, det["error"]),
                          file=sys.stderr, flush=True)
        except Exception as e:  # noqa: BLE001
            print("[llama_server] ensure: %s" % e, file=sys.stderr, flush=True)

    threading.Thread(target=job, daemon=True).start()


# ==================== CLI ====================

def _selected_roles(arg):
    if arg in (None, "all"):
        return ROLES
    if arg not in ROLES:
        raise SystemExit("Неизвестная роль '%s' (доступны: %s, all)"
                         % (arg, ", ".join(ROLES)))
    return (arg,)


def main(argv=None):
    import argparse

    parser = argparse.ArgumentParser(
        prog="hds.llama_server",
        description="Менеджер llama-server (llama.cpp) — локального "
                    "LLM-бэкенда hermes-disk-search")
    sub = parser.add_subparsers(dest="command", required=True)
    # запуск из UI/ярлыка не должен рождать консольное окно (Windows)
    _nw = {"creationflags": subprocess.CREATE_NO_WINDOW} \
        if sys.platform == "win32" else {}
    for name, help_ in (
            ("check", "живой llama-server роли на порту? (exit 0/1)"),
            ("start", "проверить порт и запустить инстанс(ы)"),
            ("stop", "остановить инстанс(ы) по PID-файлам"),
            ("status", "JSON-статус (слоты, pid, модель, контекст)"),
            ("restart", "stop + start"),
            ("run", "запустить роль в foreground (для автозапуска ОС)")):
        p = sub.add_parser(name, help=help_)
        p.add_argument("role", nargs="?", default="all")
    args = parser.parse_args(argv)
    from .config import load

    cfg = load()
    if args.command == "run":
        role = _selected_roles(args.role)[0]  # ровно одна роль
        cmd = build_command(cfg, role)
        print("llama-server (%s, foreground): %s" % (role, " ".join(cmd)),
              flush=True)
        if sys.platform == "win32":
            return subprocess.call(cmd, **_nw)
        os.execvp(cmd[0], cmd)  # заменяем процесс: сигналы launchd родные

    roles = _selected_roles(args.role)
    if args.command == "check":
        det = probe(cfg, roles[0])
        print("%s (%s): state=%s total_slots=%s"
              % (base_url(cfg, roles[0]), roles[0], det["state"],
                 det["total_slots"]))
        return 0 if det["state"] == STATE_LLAMA else 1
    if args.command == "stop":
        ok = all(stop(cfg, role) for role in roles)
        return 0 if ok else 1
    if args.command == "status":
        print(json.dumps({r: status(cfg, r) for r in roles},
                         ensure_ascii=False, indent=2))
        return 0
    if args.command == "restart":
        for role in roles:
            stop(cfg, role)
    info = ensure(cfg, roles)
    failed = [r for r, det in info.items() if "error" in det]
    print(json.dumps(info, ensure_ascii=False, indent=2))
    return 1 if failed else 0


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())