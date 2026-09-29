"""Спайк 6(г): численный паритет движка и текущего llama-server (W0 §4 п.8).

На ОДНИХ И ТЕХ ЖЕ входах:
  * embeddings: движок (`bridge embed`) vs llama-server :8011/v1/embeddings → косинус (цель ≥ 0,999);
  * rerank:     движок (`bridge rerank`) vs llama-server :8012/v1/rerank → порядок;
  * chat:       движок (reasoning="off") vs llama-server enable_thinking=false — только при
                HDS_PARITY_CHAT=1 (грузит чат-модель, нужна свободная VRAM).

Результат — out/spike6_parity.json.
"""
import glob
import json
import math
import os
import subprocess
import time

import requests

BASE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(BASE, "out")
GOLDEN = os.path.join(BASE, "golden")
ENGINE = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "TranscribeOffline", "Engine")
CLI = os.path.join(ENGINE, "example-cli.exe")
LLAMA = os.path.join(os.environ["LOCALAPPDATA"], "llama-runtime", "models")
BGE = os.path.join(LLAMA, "embedding", "bge-m3-Q8_0.gguf")
RRK = os.path.join(LLAMA, "rerank", "bge-reranker-v2-m3-q8_0.gguf")
Q6 = os.path.join(LLAMA, "chat", "Qwen3.5-9B-Q6_K.gguf")
N_EMB = 50


def engine_cli(args, timeout=900):
    env = dict(os.environ)
    env["PATH"] = ENGINE + os.pathsep + env["PATH"]
    r = subprocess.run([CLI] + args, cwd=ENGINE, capture_output=True, timeout=timeout,
                       env=env)
    return ((r.stdout or b"").decode("utf-8", "replace"),
            (r.stderr or b"").decode("utf-8", "replace"), r.returncode)


def json_from(text):
    """Первый JSON-объект в выводе CLI (перед ним печатается строка embed_start)."""
    start = text.find("{")
    while start != -1:
        try:
            obj, _ = json.JSONDecoder().raw_decode(text[start:])
            return obj
        except ValueError:
            start = text.find("{", start + 1)
    return None


def cosine(a, b):
    dot = sum(x * y for x, y in zip(a, b))
    na = math.sqrt(sum(x * x for x in a))
    nb = math.sqrt(sum(y * y for y in b))
    return dot / (na * nb + 1e-12)


def sample_texts(n=N_EMB):
    """Чанки малых фикстур (гигантские csv/лог исключаем)."""
    texts = []
    for path in sorted(glob.glob(os.path.join(GOLDEN, "*.chunks.json"))):
        name = os.path.basename(path)
        if name.startswith("большой_реестр") or name.startswith("журнал_обработки"):
            continue
        data = json.load(open(path, encoding="utf-8"))
        texts.extend(ch["text"] for ch in data["chunks"])
    if len(texts) < n:
        import sqlite3
        uri = "file:D:/hermes-disk-search-db/index.db?mode=ro"
        try:
            conn = sqlite3.connect(uri, uri=True)
            rows = conn.execute(
                "SELECT text FROM chunks WHERE length(text) BETWEEN 200 AND 900 LIMIT ?",
                (n * 2,)).fetchall()
            conn.close()
            texts.extend(r[0] for r in rows)
        except Exception as e:  # noqa: BLE001
            print("[warn] не удалось добрать тексты из БД: %s" % e)
    return texts[:n]


def http_embed(texts):
    r = requests.post("http://127.0.0.1:8011/v1/embeddings",
                      json={"model": "text-embedding-bge-m3", "input": texts},
                      timeout=600)
    r.raise_for_status()
    data = sorted(r.json()["data"], key=lambda d: d.get("index", 0))
    return [d["embedding"] for d in data]
def main():
    os.makedirs(OUT, exist_ok=True)
    res = {"generated": time.strftime("%Y-%m-%d %H:%M:%S")}
    texts = sample_texts()
    print("входов для embeddings: %d" % len(texts))

    # --- embeddings ---
    t0 = time.time()
    out, err, rc = engine_cli(["bridge", "embed", "--model", BGE, "--body-json",
                               json.dumps({"input": texts}, ensure_ascii=False)])
    t_engine = time.time() - t0
    eng = json_from(out)
    if rc != 0 or not eng or "data" not in eng:
        res["embeddings"] = {"error": "движок не вернул векторы", "rc": rc,
                             "stderr_tail": err[-500:]}
        print("embeddings: ОШИБКА движка (rc=%s)" % rc)
    else:
        ev = [d["embedding"] for d in sorted(eng["data"], key=lambda d: d["index"])]
        t1 = time.time()
        lv = http_embed(texts)
        t_ls = time.time() - t1
        cos = [cosine(a, b) for a, b in zip(ev, lv)]
        res["embeddings"] = {
            "n": len(cos), "cos_min": round(min(cos), 6),
            "cos_mean": round(sum(cos) / len(cos), 6), "cos_max": round(max(cos), 6),
            "verdict": "ПАРИТЕТ (>=0,999)" if min(cos) >= 0.999 else "РАСХОЖДЕНИЕ",
            "engine_sec": round(t_engine, 2), "llama_server_sec": round(t_ls, 2),
            "dim": len(ev[0]) if ev else 0,
        }
        print("embeddings: cos_min=%.6f cos_mean=%.6f -> %s (движок %.1f с, llama-server %.1f с)"
              % (res["embeddings"]["cos_min"], res["embeddings"]["cos_mean"],
                 res["embeddings"]["verdict"], t_engine, t_ls))

    # --- rerank ---
    query = "Работа с документооборотом согласование договоров"
    docs = texts[:10]
    out, err, rc = engine_cli(["bridge", "rerank", "--model", RRK, "--body-json",
                               json.dumps({"query": query, "documents": docs},
                                          ensure_ascii=False)])
    eng = json_from(out)
    rres = {"rc": rc}
    if eng and "results" in eng:
        by = sorted(eng["results"], key=lambda r: -r["relevance_score"])
        rres["engine_order"] = [r["index"] for r in by]
        rres["engine_scores"] = {str(r["index"]): round(r["relevance_score"], 4) for r in by}
    else:
        rres["engine_error"] = err[-300:]
    try:
        r = requests.post("http://127.0.0.1:8012/v1/rerank",
                          json={"model": "bge-reranker-v2-m3", "query": query,
                                "documents": docs}, timeout=120)
        if r.status_code == 200:
            data = r.json()
            items = data.get("results") if isinstance(data, dict) else data
            if items:
                by2 = sorted(items, key=lambda d: -d.get("relevance_score", 0))
                rres["llama_server_order"] = [d["index"] for d in by2]
                rres["llama_server_scores"] = {str(d["index"]): round(
                    float(d.get("relevance_score", 0)), 4) for d in by2}
                if rres.get("engine_scores"):
                    ea = {int(k): v for k, v in rres["engine_scores"].items()}
                    la = {int(k): v for k, v in rres["llama_server_scores"].items()}
                    common = sorted(set(ea) & set(la))
                    res["rerank_score_delta"] = {str(i): round(ea[i] - la[i], 4)
                                                 for i in common}
        else:
            rres["llama_server_error"] = "HTTP %s" % r.status_code
    except Exception as e:  # noqa: BLE001
        rres["llama_server_error"] = repr(e)[:200]
    if "engine_order" in rres and "llama_server_order" in rres:
        rres["verdict"] = ("ПОРЯДОК СОВПАЛ" if rres["engine_order"] == rres["llama_server_order"]
                           else "ПОРЯДОК ОТЛИЧАЕТСЯ")
    res["rerank"] = rres
    print("rerank: %s" % json.dumps(rres, ensure_ascii=False)[:400])

    # --- chat (опционально: грузит чат-модель) ---
    if os.environ.get("HDS_PARITY_CHAT") == "1":
        prompt = "Ответь одним словом: столица России?"
        out, err, rc = engine_cli(["bridge", "chat", "--model", Q6, "--body-json",
                                   json.dumps({"prompt": prompt, "n_predict": 32,
                                               "temperature": 0.2, "reasoning": "off"},
                                              ensure_ascii=False)])
        res["chat_engine"] = {"rc": rc, "raw_tail": out[-600:], "err_tail": err[-300:]}
        try:
            r = requests.post("http://127.0.0.1:8010/v1/chat/completions",
                              json={"model": "qwen3.5-9b",
                                    "messages": [{"role": "user", "content": prompt}],
                                    "max_tokens": 32, "temperature": 0.2,
                                    "chat_template_kwargs": {"enable_thinking": False}},
                              timeout=300)
            res["chat_llama_server"] = {"status": r.status_code, "raw": r.text[:600]}
        except Exception as e:  # noqa: BLE001
            res["chat_llama_server"] = {"error": repr(e)[:200]}
        print("chat: см. out/spike6_parity.json")
    else:
        res["chat"] = "не запускался (HDS_PARITY_CHAT!=1 — нужна свободная VRAM)"

    with open(os.path.join(OUT, "spike6_parity.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1)
    print("сохранено: out/spike6_parity.json")


if __name__ == "__main__":
    main()