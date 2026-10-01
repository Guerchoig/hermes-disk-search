"""Пилот паритета индексации B-2 (10 000 файлов): генератор дерева и дифф двух БД.

Дерево создаётся **вне репозитория** (иначе `exclude_dirs: hermes-disk-search` его
исключит), текст без OCR/медиа — детерминированно и быстро. Оба прогона индексации
(Python и Rust) используют **идентичный** конфиг, отличается только `db_path`.

Запуск из корня репозитория интерпретатором проекта:
  .\\.venv\\Scripts\\python.exe tools\\parity\\pilot_parity.py gen [ROOT]
  .\\.venv\\Scripts\\python.exe tools\\parity\\pilot_parity.py compare <db_A> <db_B>

`compare` сверяет: число файлов/чанков, `chunk_count`+`content_hash` по каждому пути,
тексты чанков и лемматизированный FTS (`chunks_fts`). Печатает `PARITY: OK/DIFF`.
"""
import os
import sqlite3
import sys

ROOT_DEFAULT = r"D:\_hds_pilot"
DIRS = 100
PER_DIR = 100  # 100*100 = 10 000 файлов
EXTS = (".txt", ".md", ".log", ".csv")
LENS = (1, 3, 12, 60, 200)  # разное число чанков
WORDS = ("настройка", "проект", "документ", "отчет", "смета", "договор",
         "system", "index", "search", "vector", "embedding", "chunk")


def gen(root=ROOT_DEFAULT):
    os.makedirs(root, exist_ok=True)
    n = 0
    for d in range(DIRS):
        sub = os.path.join(root, "dir_%03d" % d)
        os.makedirs(sub, exist_ok=True)
        for f in range(PER_DIR):
            seed = d * PER_DIR + f
            ext = EXTS[seed % len(EXTS)]
            lines = LENS[seed % len(LENS)]
            name = ("файл_%04d%s" if seed % 10 == 0 else "file_%04d%s") % (seed, ext)
            body = "\n".join(
                "%s %d строка %d: %s" % (WORDS[(seed + i) % len(WORDS)], seed, i,
                                         (WORDS[(seed + i) % len(WORDS)] + " ") * (3 + seed % 5))
                for i in range(max(1, lines))) + "\n"
            with open(os.path.join(sub, name), "w", encoding="utf-8") as fh:
                fh.write(body)
            n += 1
    print("OK tree:", root, "files:", n)


def _load(db):
    c = sqlite3.connect("file:%s?mode=ro" % db.replace("\\", "/"), uri=True)
    files = {p: (cc, ch) for p, cc, ch in
             c.execute("SELECT path, chunk_count, content_hash FROM files")}
    chunks = {(p, o): t for p, o, t in c.execute(
        "SELECT f.path, ch.ord, ch.text FROM chunks ch JOIN files f ON f.id=ch.file_id")}
    fts = {(p, o): t for p, o, t in c.execute(
        "SELECT f.path, ch.ord, ft.text FROM chunks ch JOIN files f ON f.id=ch.file_id "
        "JOIN chunks_fts ft ON ft.rowid=ch.id")}
    c.close()
    return files, chunks, fts


def compare(a_db, b_db):
    fa, ca, ta = _load(a_db)
    fb, cb, tb = _load(b_db)
    print("A files=%d chunks=%d | B files=%d chunks=%d" % (len(fa), len(ca), len(fb), len(cb)))
    only_a, only_b = set(fa) - set(fb), set(fb) - set(fa)
    cc_mism = sum(1 for p in fa if p in fb and fa[p] != fb[p])
    oc_a, oc_b = set(ca) - set(cb), set(cb) - set(ca)
    text_diff = sum(1 for k in ca if k in cb and ca[k] != cb[k])
    fts_diff = sum(1 for k in ta if k in tb and ta[k] != tb[k])
    print("files only_A=%d only_B=%d | (chunk_count,hash) mism=%d" % (len(only_a), len(only_b), cc_mism))
    print("chunks only_A=%d only_B=%d text_diff=%d | fts_diff=%d" % (len(oc_a), len(oc_b), text_diff, fts_diff))
    ok = (not only_a and not only_b and not cc_mism and not oc_a and not oc_b
          and not text_diff and not fts_diff)
    print("PARITY:", "OK" if ok else "DIFF")
    return 0 if ok else 1


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "gen":
        gen(*sys.argv[2:])
    elif cmd == "compare":
        sys.exit(compare(*sys.argv[2:]))
    else:
        print(__doc__)
        sys.exit(2)
