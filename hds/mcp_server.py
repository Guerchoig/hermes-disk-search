"""MCP-сервер disk-search для Hermes: поиск, RAG-ответы, статус, индексация."""
import os
import threading

try:
    from mcp.server.mcpserver import MCPServer as _Server
except ImportError:  # mcp 1.x
    from mcp.server.fastmcp import FastMCP as _Server

from . import db as dbmod, indexer, rag, search
from .config import PROJECT_ROOT, dig, db_abs_path, load
from .embedder import make_embedder

mcp = _Server("disk-search")

_idx_lock = threading.Lock()
_idx_state = {"running": False, "last_result": ""}


def _conn(cfg):
    return dbmod.connect(db_abs_path(cfg), int(dig(cfg, "embedding.dim", 1024)))


def _emb(cfg):
    return make_embedder(cfg)


def _format_results(results):
    lines = []
    for i, r in enumerate(results, 1):
        lines.append("[%d] %s" % (i, search.format_location(r)))
        lines.append("    %s" % r["snippet"].replace("\n", " ")[:800])
    return "\n".join(lines)


@mcp.tool()
def search_local_files(query: str, limit: int = 8, kinds: str = "") -> str:
    """Гибридный (семантический + ключевой) поиск по файлам на локальных дисках.
    Возвращает фрагменты с путями, страницами и таймкодами. kinds — необязательный
    фильтр через запятую: text,pdf,docx,xlsx,pptx,mpp,image,media."""
    cfg = load()
    conn = _conn(cfg)
    try:
        res = search.search(conn, _emb(cfg), cfg, query,
                            kinds=[k.strip() for k in kinds.split(",") if k.strip()] or None,
                            limit=max(1, min(int(limit), 30)))
        if not res:
            return "Ничего не найдено по запросу: %s" % query
        return ("Найдено %d фрагментов:\n\n%s\n\n"
                "Отвечая пользователю, приводи пути файлов и номера источников [N]. "
                "Для готового ответа используй инструмент ask_my_files."
                % (len(res), _format_results(res)))
    finally:
        conn.close()


@mcp.tool()
def ask_my_files(question: str) -> str:
    """Развёрнутый ответ на свободный вопрос по локальным файлам с цитатами
    [N] и списком источников (путь, страница/таймкод)."""
    cfg = load()
    conn = _conn(cfg)
    try:
        out = rag.ask(conn, _emb(cfg), cfg, question)
        src = "\n".join(
            "[%d] %s" % (i, search.format_location(r)) for i, r in enumerate(out["sources"], 1)
        )
        return "%s\n\nИсточники:\n%s" % (out["answer"], src or "(нет)")
    finally:
        conn.close()


@mcp.tool()
def index_status() -> str:
    """Состояние индекса: сколько файлов проиндексировано, ошибки, идёт ли индексация."""
    cfg = load()
    conn = _conn(cfg)
    try:
        st = dbmod.stats(conn)
        lines = ["Индексация сейчас: %s" % ("идёт" if _idx_state["running"] else "не запущена")]
        lines.append("Файлы по типам: " + ", ".join("%s=%d" % (k or "?", n) for k, n in st["by_kind"]))
        lines.append("По статусам: " + ", ".join("%s=%d" % (k, n) for k, n in st["by_status"]))
        lines.append("Чанков: %d" % st["chunks"])
        if st["errors"]:
            lines.append("Примеры ошибок: " + "; ".join(p for p, _e in st["errors"][:5]))
        return "\n".join(lines)
    finally:
        conn.close()


@mcp.tool()
def start_indexing(full: bool = False) -> str:
    """Запустить индексацию дисков из конфига в фоне (не блокирует чат).
    full=True — принудительная переобработка всех файлов."""
    with _idx_lock:
        if _idx_state["running"]:
            return "Индексация уже идёт. Проверьте инструментом index_status."
        _idx_state["running"] = True

    def job():
        cfg = load()
        conn = _conn(cfg)
        try:
            sf = os.path.join(PROJECT_ROOT, "index.stop")
            if os.path.exists(sf):
                os.unlink(sf)  # leftover от прошлой остановки
            indexer.run_index(conn, _emb(cfg), cfg, full=full, prune=True)
        finally:
            conn.close()
            _idx_state["running"] = False

    threading.Thread(target=job, daemon=True).start()
    threading.Thread(target=job, daemon=True).start()
    return "Фоновая индексация запущена (%s). Прогресс — через index_status. Остановка — инструментом stop_indexing." % (
        "полная" if full else "инкрементальная")


@mcp.tool()
def stop_indexing() -> str:
    """Аккуратно остановить идущую индексацию: все уже обработанные файлы
    сохраняются, текущий файл будет дообработан при следующем запуске."""
    import os

    from .config import PROJECT_ROOT

    stop_file = os.path.join(PROJECT_ROOT, "index.stop")
    open(stop_file, "w").close()
    _idx_state["last_result"] = "stop requested"
    return ("Сигнал остановки отправлен. Индексатор завершит текущий файл и остановится; "
            "проверьте завершение через index_status.")


@mcp.tool()
def reindex_path(path: str) -> str:
    """Переиндексировать один файл или папку (например, после массового изменения)."""
    import os

    cfg = load()
    conn = _conn(cfg)
    try:
        res = indexer.reindex_path(conn, _emb(cfg), cfg, os.path.abspath(path), force=True)
        return "Обработано %d файлов:\n%s" % (
            len(res), "\n".join("%s -> %s" % (r[0], r[1] or "") for r in res[:20]))
    finally:
        conn.close()


def run():
    mcp.run(transport="stdio")