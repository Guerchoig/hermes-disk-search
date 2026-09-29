"""Сжатие гигантских золотых файлов (W0 §2): 28 МБ → ~2 МБ для коммита в git.

Большие фикстуры (>1000 чанков) дают золотые файлы по 3–9 МБ. Переводим их в gzip
с сохранением полного содержимого: паритет-сравнение читает .json.gz прозрачно
(см. compare.py: load()).
"""
import glob
import gzip
import json
import os
import shutil

BASE = os.path.dirname(os.path.abspath(__file__))
GOLDEN = os.path.join(BASE, "golden")
SIZE_LIMIT = 200 * 1024


def main():
    total_before = total_after = 0
    converted = []
    for path in sorted(glob.glob(os.path.join(GOLDEN, "*.json"))):
        size = os.path.getsize(path)
        total_before += size
        if size <= SIZE_LIMIT:
            continue
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
        dst = path + ".gz"
        with gzip.open(dst, "wt", encoding="utf-8", compresslevel=9) as f:
            json.dump(data, f, ensure_ascii=False, separators=(",", ":"))
        os.remove(path)
        total_after += os.path.getsize(dst)
        converted.append((os.path.basename(path), round(size / 1024),
                          round(os.path.getsize(dst) / 1024)))
    small = sum(os.path.getsize(p) for p in glob.glob(os.path.join(GOLDEN, "*.json")))
    total_after += small
    print("сжато файлов: %d" % len(converted))
    for name, before, after in converted:
        print("  %-40s %6d КБ -> %5d КБ" % (name, before, after))
    print("golden: %.2f МБ -> %.2f МБ" % (total_before / 1048576, total_after / 1048576))


if __name__ == "__main__":
    main()