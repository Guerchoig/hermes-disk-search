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


def _fmt_elapsed(sec):
    sec = int(sec)
    return "%02d:%02d:%02d" % (sec // 3600, sec % 3600 // 60, sec % 60)


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
    """ГЛАВНЫЙ инструмент для любых запросов «найди на этом компе/диске…» — файлы,
    документы, фильмы/видео, картинки, проекты MS Project, музыка. Гибридный
    (семантический + ключевой) поиск по индексу ВСЕХ локальных дисков: секунды даже
    на сотнях тысяч файлов. Возвращает фрагменты с путями, страницами и таймкодами.
    kinds — необязательный фильтр через запятую: text,pdf,docx,xlsx,pptx,mpp,image,media
    (фильмы/видео и музыка — "media", картинки — "image")."""
    cfg = load()
    conn = _conn(cfg)
    try:
        res = search.search(conn, _emb(cfg), cfg, query,
                            kinds=[k.strip() for k in kinds.split(",") if k.strip()] or None,
                            limit=max(1, min(int(limit), 30)))
        if not res:
            return ("Ничего не найдено по запросу: %s. Если ожидаете файлы — уточните запрос "
                    "или проверьте состояние индекса инструментом index_status." % query)
        return ("Найдено %d фрагментов:\n\n%s\n\n"
                "Отвечая пользователю, приводи пути файлов и номера источников [N]. "
                "Для готового ответа используй инструмент ask_my_files."
                % (len(res), _format_results(res)))
    finally:
        conn.close()


@mcp.tool()
def ask_my_files(question: str) -> str:
    """Ответ на свободный вопрос по СОДЕРЖИМОМУ локальных файлов («о чём этот документ»,
    «в каких проектах упоминается 1С:Документооборот») с цитатами [N] и списком
    источников (путь, страница/таймкод). Используй для вопросов «что/где/в каких
    файлах…» по данным с этого компа."""
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
        rep = getattr(indexer, "_ACTIVE_REPORTER", None)
        if rep:
            import time as _t

            with rep._lock:
                cur = rep.current
                lines.append("Прогресс: просмотрено %d, обработано %d, ошибок %d, время %s"
                             % (rep.seen_count, rep.processed_count, rep.errors,
                                _fmt_elapsed(_t.time() - rep.t0)))
                if cur:
                    import datetime
                    age = _fmt_elapsed(_t.time() - (rep.current_since or _t.time()))
                    lines.append("Сейчас: %s (%s, идёт %s)" % (cur[0], cur[1], age))
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
    # autostart llama-серверов (chat + embedding) в фоне: к моменту первого
    # ask_my_files/search_local_files они, как правило, уже подняты;
    # ошибки (нет бинаря/модели) не мешают старту MCP-сервера
    try:
        cfg = load()
        if (cfg.get("llm_server") or {}).get("autostart", True):
            from . import llama_server
            llama_server.ensure_async(cfg)
    except Exception:  # noqa: BLE001
        pass
    mcp.run(transport="stdio")