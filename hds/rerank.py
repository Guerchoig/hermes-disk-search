"""Клиент реранкера (bge-reranker-v2-m3 через llama-server, эндпоинт /v1/rerank).

LM Studio не реализует /rerank (issues lmstudio-ai#162, lms#521), поэтому
реранкер запускается отдельным процессом llama-server
(`llama-server --reranking --pooling rank --port 8012 --model <gguf>`).
По умолчанию выключен (rerank.enabled: false); при недоступности или превышении
лимита латентности — деградация без падения (используются результаты без
реранкинга). Предназначен для ask_my_files (не для интерактивного поиска).
"""
import sys
import time

import requests

from .config import dig

_disabled = False  # авто-отключение на время работы процесса после тайм-аута латентности


def _rerank_url(cfg):
    return dig(cfg, "rerank.url", "http://localhost:8012/v1").rstrip("/") + "/rerank"


def rerank_results(cfg, query, results, top_n):
    """Реранк результатов поиска: возвращает переставленный top_n или None,
    если реранкер выключен/недоступен (caller использует исходный порядок)."""
    global _disabled
    if _disabled or not results:
        return None
    url = _rerank_url(cfg)
    payload = {
        "model": dig(cfg, "rerank.model", "bge-reranker-v2-m3"),
        "query": query,
        "documents": [r["text"] for r in results],
    }
    t0 = time.perf_counter()
    try:
        r = requests.post(url, json=payload,
                          timeout=float(dig(cfg, "rerank.timeout", 30)))
        r.raise_for_status()
        data = r.json()
    except Exception as e:  # noqa: BLE001
        print("[rerank] реранкер недоступен (%s) — порядок результатов без "
              "реранкинга; включите: rerank.enabled в config.yaml" % e,
              file=sys.stderr)
        return None
    elapsed = time.perf_counter() - t0
    max_lat = float(dig(cfg, "rerank.max_latency", 15))
    if elapsed > max_lat:
        # слабый CPU: реранк 20 фрагментов дольше лимита — не тормозим каждый ответ
        _disabled = True
        print("[rerank] латентность %.1f с превышает лимит %.1f с — реранкер "
              "авто-отключён до перезапуска процесса" % (elapsed, max_lat),
              file=sys.stderr)
        return None
    ranked = data.get("results") or data.get("data") or []
    order = []
    for item in ranked:
        try:
            order.append((int(item["index"]), float(item.get("relevance_score") or 0.0)))
        except (KeyError, TypeError, ValueError):
            continue
    if not order:
        return results[:top_n]
    order.sort(key=lambda kv: -kv[1])
    out = []
    for idx, _score in order:
        if 0 <= idx < len(results):
            out.append(results[idx])
        if len(out) >= top_n:
            break
    return out or results[:top_n]