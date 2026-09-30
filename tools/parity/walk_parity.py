"""Паритет обхода файлов (B1, `PLAN_W2_LLM_HOST.md` §5).

Пишет `tools/parity/out/walk_parity.json` — вход и ожидаемый результат
Python-версии для Rust-теста `crates/hds-index/tests/walk_parity.rs`:

    {"generated": "...", "scenarios": [
        {"name": "synthetic", "roots": [...], "exclude_dirs": [...],
         "exclude_paths": [...], "limits": {"max_file_mb": 1, "max_media_mb": 2},
         "files": [...],                       # что отдал hds.indexer.iter_files
         "precheck": {"<путь>": "Ok"|"SkippedType"|"SkippedExcluded"|"SkippedBig"}}]}

Сценарий `synthetic` строится тут же (`out/walk_tree`) и покрывает все ветки:
* `exclude_dirs` по имени, включая другой регистр (`NODE_MODULES`);
* `exclude_paths` по границе компонента (`backup` исключён, `backup2` — нет);
* подкаталог с исключённым именем в глубине дерева (не обходится);
* виды файлов: text/pdf/jpg/mpp/docx/mp4/неизвестный;
* `~$`-локи Office;
* лимиты: обычный файл больше `max_file_mb`, медиа больше `max_media_mb`.

Сценарий `real` (флаг `--real` или `--roots`) — боевые корни из `config.yaml`:
сильнейшая проверка (исключения, недоступные каталоги, реальные имена).

Запуск:  .\\.venv\\Scripts\\python.exe tools\\parity\\walk_parity.py [--real]
"""
import argparse
import json
import os
import shutil
import sys
import time

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
OUT = os.path.join(BASE, "out")
TREE = os.path.join(OUT, "walk_tree")
sys.path.insert(0, ROOT)

from hds.config import dig, load                                            # noqa: E402
from hds.indexer import _kind_of, _limit_mb, iter_files, path_excluded      # noqa: E402,F401

MB = 1024 * 1024

# (относительный путь, размер в МБ) — синтетическое дерево
TREE_FILES = [
    (r"ok\readme.md", 0.01),
    (r"ok\report.pdf", 0.20),
    (r"ok\clip.mp3", 1.50),
    (r"ok\video.mp4", 3.00),
    (r"ok\photo.jpg", 0.30),
    (r"ok\plan.mpp", 0.05),
    (r"ok\схема.docx", 0.10),
    (r"ok\unknown.xyz", 0.01),
    (r"ok\~$таблица.xlsx", 0.01),
    (r"big_report.txt", 1.20),
    (r"node_modules\lib.js", 0.01),
    (r"NODE_MODULES\lib2.js", 0.01),
    (r".git\config", 0.01),
    (r"__pycache__\x.pyc", 0.01),
    (r"backup\old.txt", 0.02),
    (r"backup2\new.txt", 0.02),
    (r"sub\backup\nested.txt", 0.02),
    (r"sub\deep\notes.txt", 0.02),
    (r".hidden\secret.txt", 0.02),
]


def build_tree():
    """Создать синтетическое дерево (идемпотентно)."""
    if os.path.isdir(TREE):
        shutil.rmtree(TREE)

    def mkfile(rel, mb, fill=b"\x41"):
        p = os.path.join(TREE, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(fill * max(1, int(mb * MB)))

    for rel, mb in TREE_FILES:
        mkfile(rel, mb)
    return os.path.abspath(TREE)


def precheck(cfg):
    """Порт предполётных проверок `_extract_file`: вид → exclude → `~$` → лимит."""
    def check(path):
        kind = _kind_of(os.path.splitext(path)[1].lower())
        if not kind:
            return "SkippedType"
        if path_excluded(path, cfg):
            return "SkippedExcluded"
        if os.path.basename(path).startswith("~$"):
            return "SkippedType"
        try:
            size = os.path.getsize(path)
        except OSError:
            return "SkippedStat"
        if size > _limit_mb(kind, cfg):
            return "SkippedBig"
        return "Ok"
    return check


def scenario(name, roots, cfg):
    """Собрать ожидания Python-версии для одного набора корней."""
    if roots is None:
        roots = dig(cfg, "index.roots", []) or []
    files = [os.path.abspath(p) for p in iter_files(cfg, roots)]
    chk = precheck(cfg)
    return {
        "name": name,
        "roots": [os.path.abspath(r) for r in roots],
        "exclude_dirs": [str(e) for e in dig(cfg, "index.exclude_dirs", []) or []],
        "exclude_paths": [str(e) for e in dig(cfg, "index.exclude_paths", []) or []],
        "limits": {
            "max_file_mb": int(dig(cfg, "index.max_file_mb", 200)),
            "max_media_mb": int(dig(cfg, "index.max_media_mb", 2500)),
        },
        "files": files,
        "precheck": {p: chk(p) for p in files},
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--real", action="store_true",
                    help="добавить сценарий с корнями из config.yaml")
    ap.add_argument("--roots", default="",
                    help="свои корни через ';' (сценарий custom)")
    ap.add_argument("--out", default=os.path.join(OUT, "walk_parity.json"))
    args = ap.parse_args()

    os.makedirs(OUT, exist_ok=True)
    root = build_tree()

    # лимиты сценария: уменьшенные, чтобы лимиты проверялись без гигабайтных файлов
    syn_cfg = {
        "index": {
            "exclude_dirs": ["node_modules", ".git", "__pycache__"],
            "exclude_paths": [os.path.join(root, "backup")],
            "max_file_mb": 1,
            "max_media_mb": 2,
        }
    }
    scenarios = [scenario("synthetic", [root], syn_cfg)]
    print("synthetic: %d файлов" % len(scenarios[-1]["files"]))

    cfg = load(os.path.join(ROOT, "config.yaml"))
    if args.roots:
        scenarios.append(scenario("custom", [p for p in args.roots.split(";") if p], cfg))
    elif args.real:
        scenarios.append(scenario("real", None, cfg))
    if len(scenarios) > 1:
        print("%s: %d файлов" % (scenarios[-1]["name"], len(scenarios[-1]["files"])))

    with open(args.out, "w", encoding="utf-8") as f:
        json.dump({"generated": time.strftime("%Y-%m-%d %H:%M:%S"),
                   "scenarios": scenarios}, f, ensure_ascii=False, indent=1)
    print("записано: %s (сценариев: %d)" % (args.out, len(scenarios)))

    # Пофайлово: синтетический сценарий маленький и коммитится (быстрый тест
    # у всех), «боевой» — тяжёлый (десятки тысяч путей) и в git не идёт.
    for sc in scenarios:
        dst = os.path.join(OUT, "walk_parity_%s.json" % sc["name"])
        with open(dst, "w", encoding="utf-8") as f:
            json.dump({"generated": time.strftime("%Y-%m-%d %H:%M:%S"),
                       "scenarios": [sc]}, f, ensure_ascii=False, indent=1)
        print("записано: %s (%d путей)" % (dst, len(sc["files"])))


if __name__ == "__main__":
    main()
