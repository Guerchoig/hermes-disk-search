"""CLI: index, search, ask, status, check, watch, reindex, forget, serve."""
import argparse
import json
import os
import sys
import time

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
    if args.rechunk:
        indexer.run_rechunk(conn, emb, cfg, progress_sec=args.progress_sec)
        return 0
    indexer.run_index(
        conn, emb, cfg, roots=roots, kinds=_kinds(args.kinds), full=args.full,
        limit=args.limit, confirm_delete=args.confirm_delete,
        prune=not args.no_prune,
        progress_sec=args.progress_sec, quiet=args.quiet,
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


def cmd_reindex_fts(args):
    """Перестроить chunks_fts из chunks.text (лемматизация; без эмбеддингов/OCR/AV).

    Нужна после обновления на версию с русской морфологией FTS: старые записи
    FTS содержат исходные словоформы, лемматизированный запрос их не находит.
    Watcher может параллельно писать в БД — каждая запись ждёт write-lock
    до busy_timeout (10 мин) и при неудаче повторяется; надёжнее остановить
    watcher в веб-интерфейсе на время прогона."""
    import sqlite3

    from . import lemmatizer
    from .progress import ProgressReporter

    cfg = load()
    conn = _conn(cfg)
    # 10 минут: DELETE 553 тыс. строк и батчи вставок терпеливо ждут write-lock,
    # который периодически держит watcher (30 с не хватало — «database is locked»)
    conn.execute("PRAGMA busy_timeout=600000")
    total = conn.execute("SELECT COUNT(*) FROM chunks").fetchone()[0]
    if not total:
        print("В индексе нет чанков — перестраивать нечего.")
        return 0
    print("Перестройка FTS: %d чанков%s" % (
        total, " (лемматизация pymorphy3)" if lemmatizer.available()
        else " (pymorphy3 не установлен — БЕЗ морфологии)"))
    for attempt in (1, 2, 3):
        try:
            conn.execute("DELETE FROM chunks_fts")
            conn.commit()
            break
        except sqlite3.OperationalError as e:
            if attempt == 3 or "locked" not in str(e).lower():
                raise
            print("[warn] база занята другим процессом (watcher пишет) — "
                  "повтор через 30 с (%d/3)" % attempt, flush=True)
            time.sleep(15)
    rep = ProgressReporter(args.progress_sec)
    rep.start()
    n, batch, t0 = 0, [], time.time()
    for cid, text in conn.execute("SELECT id, text FROM chunks ORDER BY id"):
        batch.append((cid, lemmatizer.normalize(text)))
        if len(batch) >= 500:
            conn.executemany("INSERT INTO chunks_fts(rowid, text) VALUES(?,?)", batch)
            conn.commit()
            n += len(batch)
            batch.clear()
            rep.seen()
    if batch:
        conn.executemany("INSERT INTO chunks_fts(rowid, text) VALUES(?,?)", batch)
        conn.commit()
        n += len(batch)
    rep.finish({"chunks": n, "elapsed_sec": round(time.time() - t0, 1)})
    # отметка «FTS перестроен под лемматизацию» — для предупреждения в check/UI
    conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES('fts_normalized','1')")
    conn.commit()
    return 0


def cmd_forget(args):
    cfg = load()
    conn = _conn(cfg)
    ok = dbmod.remove_path(conn, os.path.abspath(args.path))
    print("Удалено из индекса" if ok else "Файл в индексе не найден")
    return 0


def cmd_clip_index(args):
    """Дозаполнить CLIP-векторы для всех проиндексированных картинок."""
    cfg = load()
    conn = _conn(cfg)
    conn.execute("PRAGMA busy_timeout=30000")  # параллельно пишет watcher
    from . import clip_index

    if not clip_index.available():
        print("[--] CLIP недоступен: пакет sentence-transformers или веса модели "
              "не загрузились (см. ошибки выше)")
        return 1
    rows = [r for r in conn.execute(
        "SELECT f.id, f.path FROM files f WHERE f.kind='image' AND f.status='indexed' "
        "AND f.id NOT IN (SELECT rowid FROM images_vec)")
        if os.path.exists(r[1])]
    total = len(rows)
    print("[clip] картинок без векторов: {}".format(total))
    import struct
    import time as _t

    batch = 32
    done = 0
    i = 0
    while i < total:
        chunk = rows[i:i + batch]
        vecs = clip_index.embed_images([r[1] for r in chunk])
        for attempt in range(5):
            try:
                for (fid, _p), v in zip(chunk, vecs):
                    clip_index.add_image_vector(conn, fid, struct.pack("<%df" % len(v), *v))
                conn.commit()
                break
            except Exception as e:  # noqa: BLE001
                wait = 5 * (attempt + 1)
                print("\n[clip] БД занята, повтор через {} с: {}".format(wait, e), flush=True)
                _t.sleep(wait)
        done += len(chunk)
        i += batch
        print("\r[clip] {}/{}".format(done, total), end="", flush=True)
    print("\n[clip] готово")
    return 0


def cmd_check(args):
    from .config import ensure_config
    from .diag import has_failures, run_checks

    ensure_config()  # config.yaml может отсутствовать — создадим дефолтный
    print("== hermes-disk-search: проверка окружения ==")
    checks = run_checks()
    for c in checks:
        if c["status"] == "ok":
            print("[ok] %s" % c["title"])
            continue
        print("[%s] %s" % ("!!" if c["status"] == "fail" else "--", c["title"]))
        if c["msg"]:
            print("     %s" % c["msg"])
        if c["fix"]:
            print("     -> %s" % c["fix"])
    ok = not has_failures(checks)
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


def cmd_whisper_check(args):
    """Ручная загрузка/проверка модели Whisper (скачивание через curl + CUDA/CPU)."""
    from .extract_av import _get_whisper

    cfg = load()
    m = _get_whisper(cfg)
    print("Модель Whisper готова:", m)
    return 0


def cmd_vulkan_setup(args):
    """Установка whisper.cpp (Vulkan-сборка + GGML-веса) — GPU-ускорение
    транскрипции на AMD/Intel видеокартах, где CUDA недоступна."""
    from .config import ensure_config
    from .whisper_cpp import download_backend

    ensure_config()
    ok, msg = download_backend(load())
    print(("[ok] " if ok else "[!!] ") + msg)
    if ok:
        from .extract_av import _pick_device
        dev, _comp = _pick_device(load())
        name = {"cuda": "CUDA",
                "vulkan": "Vulkan (whisper.cpp)",
                "metal-mlx": "Metal (mlx-whisper)"}.get(dev, "CPU")
        print("[ok] Декодирование аудио/видео будет использовать: %s" % name)
    return 0 if ok else 1


def cmd_db_move(args):
    """Атомарный перенос индексной БД на новый путь (см. hds/dbops.py)."""
    from .config import PROJECT_ROOT
    from .dbops import move_db

    py = (os.path.join(PROJECT_ROOT, ".venv", "Scripts", "pythonw.exe") if os.name == "nt"
          else os.path.join(PROJECT_ROOT, ".venv", "bin", "python"))
    res = move_db(os.path.abspath(args.to), force=args.force, project=PROJECT_ROOT,
                  venv_pythonw=py)
    print(res["msg"])
    return 0 if res["ok"] else 1


def cmd_ui(args):
    """Локальный веб-интерфейс (браузер открывается автоматически)."""
    from . import ui_server
    from .config import ensure_config
    ensure_config()  # UI обязан запускаться даже без config.yaml
    ui_server.run(port=args.port, open_browser=not args.no_browser)
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
    pi.add_argument("--rechunk", action="store_true",
                    help="перечанковать проиндексированные текстовые файлы структурным "
                         "чанкером без OCR/транскрипции (переэмбеддинг затронутых файлов)")
    pi.add_argument("--limit", type=int, help="обработать не более N файлов")
    pi.add_argument("--no-prune", action="store_true", help="не удалять исчезнувшие файлы")
    pi.add_argument("--confirm-delete", action="store_true")
    pi.add_argument("--progress-sec", type=int, default=3,
                    help="частота обновления живого статуса, сек (0 = отключить)")
    pi.add_argument("--quiet", action="store_true", help="минимум вывода")
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

    pci = sub.add_parser("clip-index",
                         help="дозаполнить CLIP-векторы картинок (поиск по содержанию)")
    pci.set_defaults(fn=cmd_clip_index)

    prf = sub.add_parser("reindex-fts",
                         help="перестроить FTS-полнотекст (лемматизация, без переэмбеддинга)")
    prf.add_argument("--progress-sec", type=int, default=3,
                     help="частота обновления живого статуса, сек (0 = отключить)")
    prf.set_defaults(fn=cmd_reindex_fts)

    pck = sub.add_parser("check", help="диагностика окружения")
    pck.set_defaults(fn=cmd_check)

    pst = sub.add_parser("stop", help="аккуратно остановить идущую индексацию")
    pst.set_defaults(fn=cmd_stop)

    pwc = sub.add_parser("whisper-check", help="ручная загрузка/проверка модели Whisper")
    pwc.set_defaults(fn=cmd_whisper_check)

    pvk = sub.add_parser("vulkan-setup",
                         help="установка whisper.cpp (Vulkan) — GPU-ускорение для AMD/Intel")
    pvk.set_defaults(fn=cmd_vulkan_setup)

    pui = sub.add_parser("ui", help="веб-интерфейс (настройки, управление индексацией)")
    pui.add_argument("--port", type=int, default=8765)
    pui.add_argument("--no-browser", action="store_true")
    pui.set_defaults(fn=cmd_ui)

    pdb = sub.add_parser("db-move", help="перенос индексной БД на новый путь")
    pdb.add_argument("--to", required=True, help="новый путь к index.db")
    pdb.add_argument("--force", action="store_true", help="перезаписать существующий файл")
    pdb.set_defaults(fn=cmd_db_move)

    psv = sub.add_parser("serve", help="MCP-сервер для Hermes (stdio)")
    psv.set_defaults(fn=cmd_serve)

    args = p.parse_args(argv)
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())