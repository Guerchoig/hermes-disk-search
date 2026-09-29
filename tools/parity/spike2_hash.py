"""Спайк 2 (W0 §4 п.4): эталон content_hash для паритета с Rust.

Python-реализация (hds/indexer.py:content_hash) вычисляется на 50 файлах реального
дерева (в том числе > 512 КБ) и пишется в tools/parity/out/hash_parity.jsonl:
  {"path": ..., "size": ..., "expected": "<hex blake2b-16>"}

Затем Rust-бинарь tools/parity/spikes (cargo run --bin hash_parity) пересчитывает
те же файлы; критерий спайка — 50/50 совпадений hex.
"""
import json
import os
import sys

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
OUT = os.path.join(BASE, "out")
sys.path.insert(0, ROOT)

from hds.config import dig, load            # noqa: E402
from hds.indexer import content_hash        # noqa: E402

N_REPO, N_DISK = 25, 25


def collect_repo_files(limit):
    """Файлы репозитория/test_data (без .venv/.git), включая крупные > 512 КБ."""
    out, skip_dirs = [], {".git", ".venv", "__pycache__", "target", "node_modules"}
    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [d for d in dirnames if d not in skip_dirs]
        for fn in filenames:
            p = os.path.join(dirpath, fn)
            try:
                sz = os.path.getsize(p)
            except OSError:
                continue
            if sz:
                out.append((p, sz))
    out.sort()
    big = [x for x in out if x[1] > 512 * 1024]
    small = [x for x in out if x[1] <= 512 * 1024]
    picked, seen = [], set()
    # чередуем большие и малые, добирая до limit
    for a, b in zip(big, small):
        for x in (a, b):
            if x[0] not in seen and len(picked) < limit:
                picked.append(x)
                seen.add(x[0])
    return picked


def collect_disk_files(cfg, limit):
    """Файлы из корней индексации с учётом exclude (реальное рабочее дерево)."""
    from hds.indexer import iter_files
    picked, n = [], 0
    for p in iter_files(cfg):
        try:
            sz = os.path.getsize(p)
        except OSError:
            continue
        if 0 < sz < 50 * 1024 * 1024:      # разумный размер для быстрого замера
            picked.append((p, sz))
            n += 1
            if n >= limit:
                break
    return picked


def main():
    os.makedirs(OUT, exist_ok=True)
    cfg = load(os.path.join(ROOT, "config.yaml"))
    files = collect_repo_files(N_REPO) + collect_disk_files(cfg, N_DISK)
    files = files[:50]
    dst = os.path.join(OUT, "hash_parity.jsonl")
    with open(dst, "w", encoding="utf-8") as f:
        n_ok = 0
        for p, sz in files:
            h = content_hash(p, sz)
            if h is None:
                continue
            f.write(json.dumps({"path": p, "size": sz, "expected": h},
                               ensure_ascii=False) + "\n")
            n_ok += 1
    big = sum(1 for _, sz in files if sz > 512 * 1024)
    print("Записано %d файлов (%d > 512 КБ) -> %s" % (n_ok, big, dst))


if __name__ == "__main__":
    main()