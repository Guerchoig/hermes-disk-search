"""Фиксированные векторы паритета `content_hash` (B2, `PLAN_W2_LLM_HOST.md` §5).

Пишет `tools/parity/out/hash_vectors.json`:

    {"generated": "...", "vectors": [
        {"name": "bytes_0", "len": 0, "pattern": "i%251", "expected": "<hex>"}, ...]}

Байты воспроизводятся в Rust один-в-один: `content[i] = i % 251`. Список длин
покрывает все ветки Python-реализации: пустой файл, меньше окна, ровно окно,
между окном и 2×окном, больше 2×окном (голова и хвост перекрываются).

Запуск:  .\\.venv\\Scripts\\python.exe tools\\parity\\hash_vectors.py
"""
import json
import os
import sys
import time

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
OUT = os.path.join(BASE, "out")
sys.path.insert(0, ROOT)

from hds.indexer import content_hash  # noqa: E402

WINDOW = 256 * 1024
LENS = [
    0,
    1,
    1024,
    WINDOW - 1,
    WINDOW,
    WINDOW + 1,
    WINDOW * 2 - 1,
    WINDOW * 2,
    WINDOW * 3 + 7,
]


def write_bytes(path, n):
    with open(path, "wb") as f:
        f.write(bytes((i % 251) for i in range(n)))


def main():
    os.makedirs(OUT, exist_ok=True)
    tmp_dir = os.path.join(OUT, "hash_vectors")
    os.makedirs(tmp_dir, exist_ok=True)
    vectors = []
    for n in LENS:
        p = os.path.join(tmp_dir, "bytes_%d.bin" % n)
        write_bytes(p, n)
        h = content_hash(p, n)
        assert h is not None, p
        vectors.append({"name": "bytes_%d" % n, "len": n, "pattern": "i%251",
                        "expected": h})
        print("%-18s %8d байт -> %s" % (os.path.basename(p), n, h))
    dst = os.path.join(OUT, "hash_vectors.json")
    with open(dst, "w", encoding="utf-8") as f:
        json.dump({"generated": time.strftime("%Y-%m-%d %H:%M:%S"),
                   "window": WINDOW, "vectors": vectors}, f,
                  ensure_ascii=False, indent=1)
    print("записано: %s (%d векторов)" % (dst, len(vectors)))


if __name__ == "__main__":
    main()
