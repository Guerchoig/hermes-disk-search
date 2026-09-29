"""Сравнение реализаций с золотыми файлами (W0/§12.1 плана MIGRATION_PLAN_RUST.md).

Прогон новой (Rust) реализации по тем же фикстурам должен выдать артефакты того же
формата в отдельном каталоге. compare.py сверяет их с tools/parity/golden/.

Строго совпасть обязаны: сегменты, чанки, FTS-текст, content_hash.
С допуском: поиск — состав топ-N обязан совпасть, порядок допускает перестановку
только между записями с равными скорами.

Запуск:
  .venv/Scripts/python.exe tools/parity/compare.py --actual <dir>
"""
import argparse
import glob
import json
import os
import sys

BASE = os.path.dirname(os.path.abspath(__file__))
GOLDEN = os.path.join(BASE, "golden")


def load(d):
    """Читает золотой/фактический файл; поддерживает gzip-варианты (slim_golden.py)."""
    if os.path.exists(d):
        with open(d, encoding="utf-8") as f:
            return json.load(f)
    gz = d + ".gz"
    if os.path.exists(gz):
        import gzip

        with gzip.open(gz, "rt", encoding="utf-8") as f:
            return json.load(f)
    raise FileNotFoundError(d)


def rkey(r):
    """Ключ результата поиска: файл + страница + таймкод."""
    return "%s|%s|%s" % (r["path"], r["page"], r["t_start"])


def cmp_search(g, a, tol_note):
    rg, ra = g["results"], a["results"]
    kg = [rkey(r) for r in rg]
    ka = [rkey(r) for r in ra]
    if kg == ka:
        return True, ""
    if sorted(kg) == sorted(ka):
        tol_note.append("%s: состав совпал, порядок отличается (проверить равные скоры)"
                        % g.get("query", "?"))
        # порядок меняется только между равными скорами — проверим
        for i, (x, y) in enumerate(zip(rg, ra)):
            if rkey(x) != rkey(y) and x["score"] != y["score"]:
                return False, "%s: порядок изменён при разных скорах (поз. %d)" % (
                    g.get("query", "?"), i)
        return True, "состав совпал, порядок — только среди равных скоров"
    return False, "%s: разный состав топ-N" % g.get("query", "?")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--golden", default=GOLDEN)
    ap.add_argument("--actual", required=True, help="каталог артефактов новой реализации")
    args = ap.parse_args()
    fails, notes = [], []
    strict_total = ok_total = 0

    # 1. строгие файлы: segments/chunks/fts/hash_manifest
    for pat in ("*.segments.json", "*.chunks.json", "*.fts.json", "hash_manifest.json"):
        names = sorted({(os.path.basename(p)[:-3] if p.endswith(".gz")
                         else os.path.basename(p))
                        for p in glob.glob(os.path.join(args.golden, pat))
                        + glob.glob(os.path.join(args.golden, pat + ".gz"))})
        for name in names:
            ap2 = os.path.join(args.actual, name)
            strict_total += 1
            if not (os.path.exists(ap2) or os.path.exists(ap2 + ".gz")):
                fails.append("ОТСУТСТВУЕТ: %s" % name)
                continue
            g, a = load(os.path.join(args.golden, name)), load(ap2)
            if g == a:
                ok_total += 1
            else:
                fails.append("НЕ СОВПАЛО (строго): %s" % name)

    # 2. поиск — с допуском
    for gp in sorted(glob.glob(os.path.join(args.golden, "search_*.json"))):
        ap2 = os.path.join(args.actual, os.path.basename(gp))
        strict_total += 1
        if not os.path.exists(ap2):
            fails.append("ОТСУТСТВУЕТ: %s" % os.path.basename(gp))
            continue
        ok, msg = cmp_search(load(gp), load(ap2), notes)
        if ok:
            ok_total += 1
            if msg:
                notes.append(msg)
        else:
            fails.append("НЕ СОВПАЛО (поиск): %s" % msg)

    print("Проверено: %d, совпало: %d, не совпало: %d" % (strict_total, ok_total, len(fails)))
    for n in notes:
        print("[допуск] %s" % n)
    for f in fails:
        print("[FAIL] %s" % f)
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()