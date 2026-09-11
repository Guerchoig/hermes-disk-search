"""CLI: index, search, ask, status, check, watch, reindex, forget, serve."""
import argparse
import json
import os
import sys

from . import db as dbmod, indexer, rag, search
from .config import dig, db_abs_path, load
from .embedder import make_embedder


def _conn(cfg):
    return dbmod.connect(db_abs_path(cfg), int(dig(cfg, "embedding.dim", 1024)))


def _emb(cfg):
    return make_embedder(cfg)


def _kinds(s):
    parts = [k.strip() for k in (s or "").split(",") if k.strip()]
    return parts or None


def cmd_index(args):
    cfg = load()
    conn, emb = _conn(cfg), _emb(cfg)
    roots = args.roots.split(";") if args.roots else None
    indexer.run_index(
        conn, emb, cfg, roots=roots, kinds=_kinds(args.kinds), full=args.full,
        limit=args.limit, confirm_delete=args.confirm_delete,
        prune=not args.no_prune,
    )
    return 0


def cmd_search(args):
    cfg = load()
    conn, emb = _conn(cfg), _emb(cfg)
    res = search.search(conn, emb, cfg, args.query, kinds=_kinds(args.kinds),
                        limit=args.limit)
    if args.json:
        print(json.dumps(res, ensure_ascii=False, indent=1, default=str))
        return 0
    if not res:
        print("Ничего не найдено.")
        return 1
    for i, r in enumerate(res, 1):
        print("\n[%d] %s  (score %.4f)" % (i, search.format_location(r), r["score"]))
        print("    %s" % r["snippet"].replace("\n", " ")[:600])
    return 0


def cmd_ask(args):
    cfg = load()
    conn, emb = _conn(cfg), _emb(cfg)
    out = rag.ask(conn, emb, cfg, args.query)
    if args.json:
        print(json.dumps(out, ensure_ascii=False, indent=1, default=str))
    else:
        print(out["answer"])
    return 0


def cmd_status(args):
    cfg = load()
    conn = _conn(cfg)
    st = dbmod.stats(conn)
    if args.json:
        print(json.dumps(st, ensure_ascii=False, indent=1, default=str))
        return 0
    print("Файлы по типам: %s" % ", ".join("%s=%d" % (k or "?", n) for k, n in st["by_kind"]))
    print("Файлы по статусам: %s" % ", ".join("%s=%d" % (k, n) for k, n in st["by_status"]))
    print("Чанков всего: %d" % st["chunks"])
    if st["last_indexed_at"]:
        import datetime
        print("Последняя индексация: %s" % datetime.datetime.fromtimestamp(st["last_indexed_at"]))
    if st["errors"]:
        print("Последние ошибки:")
        for p, e in st["errors"]:
            print("  %s :: %s" % (p, (e or "")[:120]))
    return 0

def cmd_watch(args):
    cfg = load()
    from .watcher import run_watch
    roots = args.roots.split(";") if args.roots else None
    return run_watch(cfg, roots)


def cmd_reindex(args):
    cfg = load()
    conn, emb = _conn(cfg), _emb(cfg)
    for status, _k in indexer.reindex_path(conn, emb, cfg, args.path, force=not args.no_force):
        print(status)
    return 0


def cmd_forget(args):
    cfg = load()
    conn = _conn(cfg)
    ok = dbmod.remove_path(conn, os.path.abspath(args.path))
    print("Удалено из индекса" if ok else "Файл в индексе не найден")
    return 0


def cmd_check(args):
    import shutil

    import requests as rq

    cfg = load()
    ok = True
    print("== hermes-disk-search: проверка окружения ==")
    try:
        conn = _conn(cfg)
        print("[ok] sqlite-vec загружен, БД: %s" % db_abs_path(cfg))
        conn.close()
    except Exception as e:  # noqa: BLE001
        ok = False
        print("[!!] sqlite-vec/БД: %s" % e)

    chat_url = dig(cfg, "chat.base_url", "").rstrip("/") + "/models"
    try:
        r = rq.get(chat_url, timeout=10)
        models = [m.get("id") for m in r.json().get("data", [])]
        print("[ok] чат-эндпоинт %s; модели: %s" % (chat_url, models[:10]))
        chat_model = dig(cfg, "chat.model")
        if chat_model and models and chat_model not in models:
            print("[!!] чат-модель '%s' не в списке загруженных" % chat_model)
    except Exception as e:  # noqa: BLE001
        ok = False
        print("[!!] чат-эндпоинт %s недоступен: %s" % (chat_url, e))

    emb = _emb(cfg)
    try:
        emb.ping()
        print("[ok] эмбеддинги: модель '%s' отвечает" % emb.model)
    except Exception as e:  # noqa: BLE001
        print("[!!] эмбеддинги: %s" % e)
        print("     -> скачайте '%s' в LM Studio (тип Embedding) и загрузите" % emb.model)

    from .extractors import _tesseract_ready
    if _tesseract_ready(cfg):
        print("[ok] Tesseract OCR найден")
    else:
        print("[--] Tesseract OCR не найден (картинки без OCR): winget install UB-Mannheim.TesseractOCR")

    if shutil.which("ffmpeg"):
        print("[ok] ffmpeg найден")
    else:
        print("[--] ffmpeg не найден (видео без транскрипции)")

    try:
        import faster_whisper  # noqa: F401
        print("[ok] faster-whisper установлен")
    except Exception:  # noqa: BLE001
        print("[--] faster-whisper не установлен: pip install faster-whisper")

    try:
        import mpxj  # noqa: F401
        print("[ok] mpxj (MS Project) установлен")
    except Exception:  # noqa: BLE001
        print("[--] mpxj не установлен (.mpp не парсятся): pip install mpxj + Java 11+")

    print("Итог: %s" % ("основные компоненты готовы" if ok else "есть критические проблемы"))
    return 0 if ok else 1


def cmd_stop(args):
    """Создаёт index.stop — сигнал аккуратной остановки индексации."""
    from .config import PROJECT_ROOT

    sf = os.path.join(PROJECT_ROOT, "index.stop")
    open(sf, "w").close()
    print("Сигнал остановки создан:", sf)
    print("Индексатор завершит текущий файл и остановится (обработанное сохранится).")
    return 0


def cmd_serve(args):
    from .mcp_server import run
    run()
    return 0


def main(argv=None):
    p = argparse.ArgumentParser(prog="hds", description="Поиск по локальным файлам")
    sub = p.add_subparsers(dest="cmd", required=True)

    pi = sub.add_parser("index", help="индексация дисков/папок (инкрементально)")
    pi.add_argument("--roots", help="корни через ';' (по умолчанию из конфига)")
    pi.add_argument("--kinds", help="фильтр типов: text,pdf,docx,xlsx,pptx,mpp,image,media")
    pi.add_argument("--full", action="store_true", help="принудительно переобработать всё")
    pi.add_argument("--limit", type=int, help="обработать не более N файлов")
    pi.add_argument("--no-prune", action="store_true", help="не удалять исчезнувшие файлы")
    pi.add_argument("--confirm-delete", action="store_true")
    pi.set_defaults(fn=cmd_index)

    ps = sub.add_parser("search", help="поиск по индексу")
    ps.add_argument("query")
    ps.add_argument("--limit", type=int, default=8)
    ps.add_argument("--kinds")
    ps.add_argument("--json", action="store_true")
    ps.set_defaults(fn=cmd_search)

    pa = sub.add_parser("ask", help="ответ на свободный вопрос с цитатами")
    pa.add_argument("query")
    pa.add_argument("--json", action="store_true")
    pa.set_defaults(fn=cmd_ask)

    pst = sub.add_parser("status", help="состояние индекса")
    pst.add_argument("--json", action="store_true")
    pst.set_defaults(fn=cmd_status)

    pw = sub.add_parser("watch", help="режим наблюдателя: реакция на события ФС")
    pw.add_argument("--roots", help="корни через ';'")
    pw.set_defaults(fn=cmd_watch)

    pr = sub.add_parser("reindex", help="переиндексировать файл/папку")
    pr.add_argument("path")
    pr.add_argument("--no-force", action="store_true")
    pr.set_defaults(fn=cmd_reindex)

    pf = sub.add_parser("forget", help="убрать файл из индекса")
    pf.add_argument("path")
    pf.set_defaults(fn=cmd_forget)

    pck = sub.add_parser("check", help="диагностика окружения")
    pck.set_defaults(fn=cmd_check)

    pst = sub.add_parser("stop", help="аккуратно остановить идущую индексацию")
    pst.set_defaults(fn=cmd_stop)

    psv = sub.add_parser("serve", help="MCP-сервер для Hermes (stdio)")
    psv.set_defaults(fn=cmd_serve)

    args = p.parse_args(argv)
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())