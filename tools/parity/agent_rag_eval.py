"""K1 (A6.8): сравнение агента (А) с `disk-search` и (Б) без него.

Для каждого вопроса эталона и каждой конфигурации выполняется
``hds agent run --project <P> --json --prompt "<вопрос + просьба перечислить источники>"``;
из потока событий собираются ``usage`` (вход/выход, стоимость), ``turn_end``
(``ok``/``reason``/``tools``) и ``tool_use``/``tool_result``; время хода — по часам
процесса. Сырые JSONL и сводка сохраняются в ``<out-dir>/k1-*``.

Полнота ответа (0/1/2) считается по эталонным источникам golden: имена образов
(``anon_rel`` из реестра P0001) и исходные имена (деанонимизация в ответе) — одна
и та же сущность. 2 — найдены все эталонные файлы, 1 — часть, 0 — ни одного.
Точность — доля выдуманных имён файлов среди названных в ответе; отдельно
считается покрытие по выдачам инструментов (``tool_result``).

Запуск (после `cargo build`; для (А) поднят `hds mcp-http start`):

    python tools\\parity\\agent_rag_eval.py ^
        --hds target\\debug\\hds.exe ^
        --config-a tools\\parity\\out\\agent_a42\\config.yaml ^
        --config-b tools\\parity\\out\\agent_a42\\k1-config-B.yaml ^
        --root tools\\parity\\out\\agent_a42\\root ^
        --workspace tools\\parity\\out\\agent_a42\\zone\\P0001\\workspace ^
        --golden tools\\parity\\out\\agent_a42\\golden-k1.jsonl ^
        --out-dir tools\\parity\\out\\agent_a42 ^
        --provider-label openrouter:anthropic/claude-sonnet-4.5

Повторная оценка уже снятых прогонов (без запуска агента): ``--score-only``.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.request

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
FILE_RE = re.compile(r"[0-9A-Za-zА-Яа-яЁё_\- .()\[\]]+\.(?:docx|xlsx|md|pdf|txt|pptx|mpp)", re.I)
#: Время хода по прогонам (`A-qQ01-r1` → секунды): сырые JSONL его не хранят,
#: поэтому `--score-only` берёт время отсюда (AG6-05).
TIMES_FILE = "k1-times.json"
#: «Заметный» выигрыш по полноте на вопросах «поиск по смыслу»: +0.5 из 2 (§6 плана).
SEMANTIC_GAIN = 0.5
#: Порог экономии токенов из правила решения §6.
TOKEN_SAVING = 0.25

PROMPT = (
    "{question}\n\n"
    "Ответь по документам проекта. В конце перечисли имена файлов-источников, "
    "каждый с новой строки, с расширением. Не выдумывай файлы, которых нет."
)


def basename(path: str) -> str:
    return os.path.basename(str(path).replace("\\", "/"))


def load_golden(path: str) -> list[dict]:
    questions = []
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if line:
                questions.append(json.loads(line))
    return questions


def registry_map(db: str) -> dict[str, str]:
    """`anon_rel` → `orig_rel` (basename) из реестра артефактов стенда."""
    import sqlite3

    mapping: dict[str, str] = {}
    if not os.path.isfile(db):
        return mapping
    conn = sqlite3.connect(db)
    try:
        for orig, anon in conn.execute("select orig_rel, anon_rel from artifacts"):
            if orig and anon:
                mapping[basename(anon)] = basename(orig)
    finally:
        conn.close()
    return mapping


def workspace_names(workspace: str) -> set[str]:
    names: set[str] = set()
    for _root, _dirs, files in os.walk(workspace):
        for name in files:
            names.add(name)
    return names


def cited_files(text: str) -> list[str]:
    found = []
    for match in FILE_RE.findall(text or ""):
        name = match.strip(" .()[]")
        if name and name not in found:
            found.append(name)
    return found


def hit(name: str, expected: list[str], aliases: dict[str, str]) -> bool:
    """Эталон найден в тексте: имя образа или исходное имя (деанонимизация)."""
    low = name.lower()
    for item in expected:
        if item.lower() in low:
            return True
        orig = aliases.get(item)
        if orig and orig.lower() in low:
            return True
    return False
def score_run(expected: list[str], answer: str, tool_output: str,
              aliases: dict[str, str], known: set[str]) -> dict:
    known_all = known | set(aliases.values())
    # Кандидат из прозы («ответ. Протокол 06 …docx») канонизируется до имени файла.
    cited: list[str] = []
    fabricated: list[str] = []
    for candidate in cited_files(answer):
        low = candidate.lower()
        match = next((name for name in sorted(known_all, key=len, reverse=True)
                      if name.lower() in low), None)
        if match:
            if match not in cited:
                cited.append(match)
        elif candidate not in fabricated:
            fabricated.append(candidate)
    found_answer = [item for item in expected if hit(answer, [item], aliases)]
    found_tools = [item for item in expected if hit(tool_output, [item], aliases)]
    coverage = len(found_answer) / max(1, len(expected))
    if coverage >= 0.999:
        completeness = 2
    elif coverage > 0:
        completeness = 1
    else:
        completeness = 0
    return {
        "completeness": completeness,
        "coverage": round(coverage, 3),
        "foundAnswer": found_answer,
        "foundTools": found_tools,
        "missing": [item for item in expected if item not in found_answer],
        "cited": cited,
        "fabricated": fabricated,
        "precision": round(1.0 - len(fabricated) / (len(cited) + len(fabricated)), 3)
        if (cited or fabricated) else None,
    }


def parse_lines(text: str) -> tuple[list[dict], str]:
    events, noise = [], []
    for raw in text.splitlines():
        line = raw.strip().lstrip("\ufeff")
        if not line:
            continue
        if not line.startswith("{"):
            noise.append(line)
            continue
        try:
            events.append(json.loads(line))
        except json.JSONDecodeError:
            noise.append(line)
    return events, "\n".join(noise)


def summarize_events(events: list[dict]) -> dict:
    usage_in = usage_out = 0
    cost = 0.0
    has_cost = False
    tools_used: list[str] = []
    tool_errors = 0
    tool_output: list[str] = []
    turn_end: dict = {}
    text_parts: list[str] = []
    for item in events:
        kind = item.get("kind")
        if kind == "agent":
            event = item.get("event") or {}
            ekind = event.get("kind")
            if ekind == "usage":
                usage_in += int(event.get("input_tokens") or 0)
                usage_out += int(event.get("output_tokens") or 0)
                if event.get("total_cost") is not None:
                    has_cost = True
                    cost += float(event.get("total_cost") or 0)
            elif ekind == "tool_use":
                tools_used.append(str(event.get("tool") or ""))
            elif ekind == "tool_result":
                if event.get("error"):
                    tool_errors += 1
                tool_output.append(json.dumps(event.get("output"), ensure_ascii=False))
            elif ekind == "text":
                text_parts.append(str(event.get("text") or ""))
        elif kind == "turn_end":
            turn_end = item
    answer = str(turn_end.get("text") or "") or "".join(text_parts)
    return {
        "usageIn": usage_in,
        "usageOut": usage_out,
        "usageTotal": usage_in + usage_out,
        "cost": round(cost, 6) if has_cost else None,
        "toolsUsed": tools_used,
        "toolErrors": tool_errors,
        "toolOutput": "\n".join(tool_output),
        "answer": answer,
        "turnEnd": {
            "ok": turn_end.get("ok"),
            "reason": turn_end.get("reason"),
            "error": turn_end.get("error"),
            "tools": turn_end.get("tools"),
        },
    }
#: Ошибки провайдера/сети, при которых ход имеет смысл повторить (TLS-прокси,
#: обрывы, лимиты). Детерминированные ошибки (модель, ключ, фильтр) не повторяем.
TRANSIENT = re.compile(
    r"self-signed|certificate|ECONNRESET|ETIMEDOUT|EAI_AGAIN|socket hang up|"
    r"429|rate limit|\b50[0-4]\b|fetch failed|network", re.I)


def is_transient(message: str) -> bool:
    return bool(message and TRANSIENT.search(message))


def run_once(hds: str, config: str, root: str, project: str, prompt: str,
             timeout: int, max_iterations: int, raw_path: str, err_path: str,
             retries: int = 0) -> dict:
    last: dict = {}
    for attempt in range(retries + 1):
        last = run_attempt(hds, config, root, project, prompt, timeout,
                           max_iterations, raw_path, err_path)
        error = (last.get("turnEnd") or {}).get("error") or ""
        if last.get("turnEnd", {}).get("ok") or not is_transient(error):
            break
        if attempt < retries:
            print(f"  retry {os.path.basename(raw_path)}: {error[:120]}", flush=True)
            time.sleep(5)
    last["attempts"] = attempt + 1
    return last


def run_attempt(hds: str, config: str, root: str, project: str, prompt: str,
                timeout: int, max_iterations: int, raw_path: str,
                err_path: str) -> dict:
    env = dict(os.environ)
    env["HDS_CONFIG"] = config
    env["HDS_ROOT"] = root
    args = [hds, "agent", "run", "--project", project, "--json",
            "--max-iterations", str(max_iterations), "--prompt", prompt]
    started = time.time()
    try:
        proc = subprocess.run(args, cwd=REPO, env=env, capture_output=True,
                              timeout=timeout)
        code = proc.returncode
        stdout = proc.stdout.decode("utf-8", errors="replace")
        stderr = proc.stderr.decode("utf-8", errors="replace")
        timed_out = False
    except subprocess.TimeoutExpired as exc:
        code = -1
        stdout = (exc.stdout or b"").decode("utf-8", errors="replace")
        stderr = (exc.stderr or b"").decode("utf-8", errors="replace")
        timed_out = True
    elapsed = time.time() - started
    with open(raw_path, "w", encoding="utf-8") as fh:
        fh.write(stdout)
    with open(err_path, "w", encoding="utf-8") as fh:
        fh.write(stderr)
    events, noise = parse_lines(stdout)
    summary = summarize_events(events)
    summary.update({
        "exit": code,
        "timedOut": timed_out,
        "seconds": round(elapsed, 2),
        "noiseLines": len(noise.splitlines()) if noise else 0,
        "raw": os.path.relpath(raw_path, REPO),
        "stderr": os.path.relpath(err_path, REPO),
    })
    return summary


def aggregate(rows: list[dict]) -> dict:
    if not rows:
        return {"n": 0}
    precisions = [r["precision"] for r in rows if r["precision"] is not None]
    return {
        "n": len(rows),
        "completenessMean": round(statistics.fmean(r["completeness"] for r in rows), 3),
        "completeness2Rate": round(sum(1 for r in rows if r["completeness"] == 2) / len(rows), 3),
        "coverageMean": round(statistics.fmean(r["coverage"] for r in rows), 3),
        "precisionMean": round(statistics.fmean(precisions), 3) if precisions else None,
        "fabricatedTotal": sum(len(r["fabricated"]) for r in rows),
        "tokensMean": round(statistics.fmean(r["usageTotal"] for r in rows), 1),
        "tokensMedian": round(statistics.median(r["usageTotal"] for r in rows), 1),
        "inMean": round(statistics.fmean(r["usageIn"] for r in rows), 1),
        "outMean": round(statistics.fmean(r["usageOut"] for r in rows), 1),
        "stepsMean": round(statistics.fmean(len(r["toolsUsed"]) for r in rows), 2),
        "secondsMean": round(statistics.fmean(r["seconds"] for r in rows
                                              if r["seconds"] is not None), 2)
        if any(r["seconds"] is not None for r in rows) else None,
        "failed": sum(1 for r in rows if not r["turnEnd"].get("ok")),
        "costMean": round(statistics.fmean(r["cost"] for r in rows), 6)
        if all(r["cost"] is not None for r in rows) else None,
    }


def load_times(out_dir: str) -> dict[str, float]:
    """Времена прогонов из `k1-times.json` (его нет — пусто)."""
    path = os.path.join(out_dir, TIMES_FILE)
    try:
        # utf-8-sig: файл могли поправить в PowerShell 5.1 (BOM).
        with open(path, encoding="utf-8-sig") as fh:
            data = json.load(fh)
    except (OSError, json.JSONDecodeError):
        return {}
    return {
        str(key): float(value)
        for key, value in data.items()
        if isinstance(value, (int, float))
    }


def save_times(out_dir: str, times: dict[str, float]) -> None:
    path = os.path.join(out_dir, TIMES_FILE)
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(times, fh, ensure_ascii=False, indent=2, sort_keys=True)


def mcp_alive(url: str) -> bool:
    try:
        with urllib.request.urlopen(url, timeout=5) as resp:  # noqa: S310 — loopback
            return resp.status == 200
    except (urllib.error.URLError, OSError):
        return False
def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--hds", default=os.path.join(REPO, "target", "debug", "hds.exe"))
    parser.add_argument("--config-a", required=True, help="конфиг (А): disk-search включён")
    parser.add_argument("--config-b", required=True, help="конфиг (Б): disk_search.enabled=false")
    parser.add_argument("--root", required=True, help="HDS_ROOT стенда")
    parser.add_argument("--workspace", required=True, help="корень образов <P>\\workspace")
    parser.add_argument("--project", default="P0001")
    parser.add_argument("--registry", default=None)
    parser.add_argument("--golden", required=True)
    parser.add_argument("--out-dir", default=None)
    parser.add_argument("--runs", type=int, default=2)
    parser.add_argument("--max-iterations", type=int, default=12)
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--retries", type=int, default=2,
                        help="повторы хода при транзиентных ошибках провайдера/сети")
    parser.add_argument("--only", choices=["A", "B"], default=None)
    parser.add_argument("--questions", default=None, help="ID через запятую (Q01,Q02)")
    parser.add_argument("--run-index", default=None,
                        help="номера прогонов через запятую (1,2) — для точечного перезапуска")
    parser.add_argument("--provider-label", default="unknown")
    parser.add_argument("--models-dir", default=None,
                        help="HDS_MODELS_DIR для воркера анонимизации (ProgramData\\...\\models)")
    parser.add_argument("--mcp-url", default="http://127.0.0.1:8788/health")
    parser.add_argument("--skip-mcp-check", action="store_true")
    parser.add_argument("--score-only", action="store_true",
                        help="только пересчитать сохранённые k1-<cfg>-q*-r*.jsonl "
                             "(время — из k1-times.json, сводка — k1-summary-score.json)")
    args = parser.parse_args()

    # hds запускает воркер с `current_dir(<root>)`: относительный `--root` даёт
    # удвоенный путь к скрипту и «поток вывода закрыт» через 0,1 с.
    args.root = os.path.abspath(args.root)
    args.workspace = os.path.abspath(args.workspace)
    args.config_a = os.path.abspath(args.config_a)
    args.config_b = os.path.abspath(args.config_b)
    out_dir = os.path.abspath(args.out_dir) if args.out_dir else os.path.join(
        REPO, "tools", "parity", "out", "agent_a42")
    os.makedirs(out_dir, exist_ok=True)
    if args.models_dir:
        # Воркер анонимизации берёт веса из `<models>/anonymize`; без этого
        # стенд без `<root>/models/anonymize` падает через 0,1 с.
        os.environ["HDS_MODELS_DIR"] = args.models_dir
    registry = args.registry or os.path.join(
        args.root, "data", "agent", "registry", f"{args.project}.db")
    aliases = registry_map(registry)
    known = workspace_names(args.workspace)
    questions = load_golden(args.golden)
    if args.questions:
        wanted = {q.strip() for q in args.questions.split(",")}
        questions = [q for q in questions if q["id"] in wanted]

    configs = [("A", args.config_a), ("B", args.config_b)]
    if args.only:
        configs = [pair for pair in configs if pair[0] == args.only]

    if not args.score_only and any(cfg == "A" for cfg, _ in configs):
        if not args.skip_mcp_check and not mcp_alive(args.mcp_url):
            print(f"нет MCP-сервера ({args.mcp_url}): поднимите `hds mcp-http start` "
                  f"или передайте --skip-mcp-check (тогда (А) пойдёт без disk-search)",
                  file=sys.stderr)
            return 2

    run_indexes = ([int(x) for x in args.run_index.split(",")] if args.run_index
                   else list(range(1, args.runs + 1)))
    times = load_times(out_dir)
    rows: list[dict] = []
    for label, config in configs:
        for question in questions:
            for run in run_indexes:
                stem = os.path.join(out_dir, f"k1-{label}-q{question['id']}-r{run}")
                raw_path, err_path = stem + ".jsonl", stem + ".err"
                key = f"{label}-q{question['id']}-r{run}"
                if args.score_only:
                    if not os.path.isfile(raw_path):
                        print(f"skip {stem}: нет файла прогона", file=sys.stderr)
                        continue
                    with open(raw_path, encoding="utf-8") as fh:
                        events, noise = parse_lines(fh.read())
                    summary = summarize_events(events)
                    summary.update({
                        "exit": None, "timedOut": False,
                        # AG6-05: время не в JSONL — берём из k1-times.json.
                        "seconds": times.get(key),
                        "noiseLines": len(noise.splitlines()) if noise else 0,
                        "raw": os.path.relpath(raw_path, REPO),
                        "stderr": os.path.relpath(err_path, REPO),
                    })
                else:
                    prompt = PROMPT.format(question=question["question"])
                    summary = run_once(args.hds, config, args.root, args.project, prompt,
                                       args.timeout, args.max_iterations, raw_path,
                                       err_path, args.retries)
                    if summary.get("seconds") is not None:
                        times[key] = float(summary["seconds"])
                row = {
                    "config": label,
                    "id": question["id"],
                    "type": question["type"],
                    "question": question["question"],
                    "expected": question["expected"],
                    "run": run,
                }
                row.update({k: v for k, v in summary.items()
                            if k not in ("toolOutput", "answer")})
                row.update(score_run(question["expected"], summary["answer"],
                                     summary["toolOutput"], aliases, known))
                rows.append(row)
                print(f"{label} {question['id']} r{run}: comp={row['completeness']} "
                      f"cov={row['coverage']} tokens={row['usageTotal']} "
                      f"steps={len(row['toolsUsed'])} {row['seconds']}s "
                      f"ok={row['turnEnd'].get('ok')}", flush=True)

    if not rows:
        print("нет данных: нечего сводить", file=sys.stderr)
        return 1
    by_config = {label: aggregate([r for r in rows if r["config"] == label])
                 for label in ("A", "B")}
    by_type = {}
    for label in ("A", "B"):
        for question_type in sorted({r["type"] for r in rows}):
            subset = [r for r in rows if r["config"] == label and r["type"] == question_type]
            if subset:
                by_type.setdefault(label, {})[question_type] = aggregate(subset)

    report = {
        "generated": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "provider": args.provider_label,
        "golden": os.path.relpath(args.golden, REPO),
        "configA": args.config_a,
        "configB": args.config_b,
        "runs": args.runs,
        "maxIterations": args.max_iterations,
        "overall": by_config,
        "byType": by_type,
        "rows": rows,
    }

    a, b = by_config.get("A", {}), by_config.get("B", {})
    if a.get("n") and b.get("n"):
        saving = (b["tokensMean"] - a["tokensMean"]) / b["tokensMean"] if b["tokensMean"] else 0.0
        semantic_a = by_type.get("A", {}).get("semantic", {}).get("completenessMean")
        semantic_b = by_type.get("B", {}).get("semantic", {}).get("completenessMean")
        semantic_gain = ((semantic_a or 0) - (semantic_b or 0)
                         if semantic_a is not None and semantic_b is not None else None)
        no_worse = a["completenessMean"] >= b["completenessMean"]
        by_tokens = no_worse and saving >= TOKEN_SAVING
        by_semantic = (semantic_gain is not None and semantic_gain >= SEMANTIC_GAIN
                       and a["completenessMean"] >= b["completenessMean"])
        report["decision"] = {
            "rule": "RAG включён, если (А) не хуже (Б) по полноте и экономит >= 25% "
                    "токенов ИЛИ заметно (>= +0.5 из 2) выигрывает на вопросах "
                    "«поиск по смыслу» (PLAN_AGENT_PRIVACY_ZONE.md §6)",
            "tokenSaving": round(saving, 3),
            "semanticGain": None if semantic_gain is None else round(semantic_gain, 3),
            "completenessNoWorse": no_worse,
            "byTokens": by_tokens,
            "bySemantic": by_semantic,
            "verdict": "keep" if (by_tokens or by_semantic) else "disable",
        }

    if not args.score_only:
        # AG6-05: времена прогонов — отдельным файлом, чтобы `--score-only`
        # воспроизводил и их (в сырых JSONL времени нет).
        save_times(out_dir, times)

    summary_path = os.path.join(
        out_dir, "k1-summary-score.json" if args.score_only else "k1-summary.json")
    with open(summary_path, "w", encoding="utf-8") as fh:
        json.dump(report, fh, ensure_ascii=False, indent=2)

    print("\n=== A vs B ===")
    for label in ("A", "B"):
        if by_config.get(label, {}).get("n"):
            print(f"  {label}: {json.dumps(by_config[label], ensure_ascii=False)}")
    if "decision" in report:
        print("=== decision ===")
        print(json.dumps(report["decision"], ensure_ascii=False))
    print(f"\nreport: {summary_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
