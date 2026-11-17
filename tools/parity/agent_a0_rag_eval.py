"""Оценка поиска по анонимным образам (S10, первая ступень методики «Оценка RAG»).

Для каждого вопроса эталона выполняет ``hds search <вопрос> --limit 30 --json`` с
временным конфигом стенда и считает ``recall@5``, ``recall@10``, ``recall@30`` и MRR по
эталонным файлам-источникам. Отдельно проверяется поиск по метке (``[PERSON_3]``):
сколько выдач содержат именно эту метку (факт для TR-4.6).

Запуск (после сборки индекса стенда):
    python tools\\parity\\agent_a0_rag_eval.py ^
        --hds target\\release\\hds.exe ^
        --config tools\\parity\\out\\agent_a0\\s10\\config.yaml ^
        --golden tools\\parity\\out\\agent_a0\\s10\\golden.jsonl ^
        --out tools\\parity\\out\\agent_a0\\s10\\eval.json
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DEFAULT_LIMIT = 30
LABEL_RE = re.compile(r"\[[A-Z_]+_\d+(?:\|[а-яё,]+)?\]")


def run_search(hds: str, config: str, query: str, limit: int) -> tuple[list[dict], float, str]:
    env = dict(os.environ)
    env["HDS_CONFIG"] = config
    env.pop("HDS_ROOT", None)
    started = time.time()
    try:
        proc = subprocess.run(
            [hds, "search", query, "--limit", str(limit), "--json"],
            cwd=REPO,
            env=env,
            capture_output=True,
            timeout=180,
        )
    except subprocess.TimeoutExpired:
        return [], (time.time() - started) * 1000, "timeout 180s"
    elapsed = (time.time() - started) * 1000
    out = proc.stdout.decode("utf-8", errors="replace")
    if proc.returncode != 0:
        err = proc.stderr.decode("utf-8", errors="replace")[:500]
        return [], elapsed, err or f"exit {proc.returncode}"
    try:
        return json.loads(out), elapsed, ""
    except json.JSONDecodeError as exc:
        return [], elapsed, f"json error: {exc}: {out[:200]}"


def basename_of(path: str) -> str:
    return os.path.basename(path.replace("\\", "/"))


def count_label_hits(results: list[dict], question: str) -> int | None:
    """Сколько из выданных фрагментов действительно содержат метку из вопроса."""
    labels = set(LABEL_RE.findall(question))
    if not labels:
        return None
    hits = 0
    for item in results:
        text = f"{item.get('snippet') or ''}\n{item.get('text') or ''}"
        if any(label in text for label in labels):
            hits += 1
    return hits


def first_rank(result_paths: list[str], expected: list[str]) -> int | None:
    """Позиция (1-based) первого эталонного файла в выдаче."""
    for index, path in enumerate(result_paths, start=1):
        name = basename_of(path)
        if name in expected:
            return index
    return None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--hds", default=os.path.join(REPO, "target", "release", "hds.exe"))
    parser.add_argument("--config", required=True)
    parser.add_argument("--golden", required=True)
    parser.add_argument("--out", default=None)
    parser.add_argument("--limit", type=int, default=DEFAULT_LIMIT)
    args = parser.parse_args()

    questions = []
    with open(args.golden, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if line:
                questions.append(json.loads(line))

    rows = []
    for question in questions:
        results, elapsed, error = run_search(args.hds, args.config, question["question"], args.limit)
        paths = [item.get("path", "") for item in results]
        rank = first_rank(paths, question["expected"])
        rows.append(
            {
                "id": question["id"],
                "type": question["type"],
                "question": question["question"],
                "expected": question["expected"],
                "returned": [basename_of(p) for p in paths[:10]],
                "returnedCount": len(paths),
                "firstRank": rank,
                "hit@1": bool(rank and rank <= 1),
                "hit@5": bool(rank and rank <= 5),
                "hit@10": bool(rank and rank <= 10),
                "hit@30": bool(rank and rank <= 30),
                "mrr": round(1.0 / rank, 4) if rank else 0.0,
                "ms": round(elapsed, 1),
                "error": error,
                # сколько из топ-10 фрагментов содержат метку из вопроса (для вопросов по меткам)
                "snippetsWithLabel": count_label_hits(results[:10], question["question"]),
            }
        )
        print(
            f"{question['id']} [{question['type']}] rank={rank} "
            f"hit@5={rows[-1]['hit@5']} {round(elapsed)}ms",
            flush=True,
        )

    def aggregate(items: list[dict]) -> dict:
        total = max(1, len(items))
        return {
            "n": len(items),
            "recall@1": round(sum(1 for r in items if r["hit@1"]) / total, 3),
            "recall@5": round(sum(1 for r in items if r["hit@5"]) / total, 3),
            "recall@10": round(sum(1 for r in items if r["hit@10"]) / total, 3),
            "recall@30": round(sum(1 for r in items if r["hit@30"]) / total, 3),
            "mrr": round(sum(r["mrr"] for r in items) / total, 3),
            "avg_ms": round(sum(r["ms"] for r in items) / total, 1),
        }

    by_type = {}
    for row in rows:
        by_type.setdefault(row["type"], []).append(row)

    by_ext = {}
    for row in rows:
        ext = os.path.splitext(row["expected"][0])[1].lower() if row["expected"] else "?"
        by_ext.setdefault(ext, []).append(row)

    # отдельный тест TR-4.6: поиск по метке как по одному слову
    label_tests = []
    for label in ["[PERSON_3]", "[ORG_21]"]:
        results, elapsed, error = run_search(args.hds, args.config, label, 10)
        snippets = [(item.get("snippet") or "") for item in results]
        label_tests.append(
            {
                "label": label,
                "returned": len(results),
                "withLabel": sum(1 for s in snippets if label in s),
                "ms": round(elapsed, 1),
                "error": error,
            }
        )

    report = {
        "golden": args.golden,
        "config": args.config,
        "limit": args.limit,
        "overall": aggregate(rows),
        "byType": {key: aggregate(value) for key, value in by_type.items()},
        "byFirstExpectedExt": {key: aggregate(value) for key, value in by_ext.items()},
        "labelSearch": label_tests,
        "rows": rows,
    }

    out_path = args.out or os.path.join(os.path.dirname(args.golden), "eval.json")
    with open(out_path, "w", encoding="utf-8") as fh:
        json.dump(report, fh, ensure_ascii=False, indent=2)

    print("\n=== overall ===")
    print(json.dumps(report["overall"], ensure_ascii=False))
    print("=== by type ===")
    for key, value in report["byType"].items():
        print(f"  {key}: recall@10={value['recall@10']} mrr={value['mrr']} n={value['n']}")
    print("=== label search ===")
    for item in label_tests:
        print(f"  {item['label']}: returned={item['returned']} withLabel={item['withLabel']}")
    print(f"\nreport: {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

