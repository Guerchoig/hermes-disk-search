"""Ответ на свободный вопрос по локальным файлам (RAG через чат-модель LM Studio)."""
import requests

from .config import dig
from .search import format_location, search

SYSTEM_PROMPT = (
    "Ты помогаешь искать информацию в локальных файлах пользователя. "
    "Отвечай на русском языке. Опирайся ТОЛЬКО на приведённые фрагменты документов; "
    "если их недостаточно — так и скажи. При упоминании фактов указывай источник в "
    "формате [N], где N — номер фрагмента. Отвечай компактно: суть в нескольких "
    "предложениях и короткий список источников (путь, страница/таймкод) в конце — "
    "ответ генерирует локальная модель, длинные ответы не помещаются в таймаут клиента."
)


def _fmt_time(t):
    return "%02d:%02d:%02d" % (t // 3600, t % 3600 // 60, t % 60)


def build_context(results, max_chars):
    blocks, used = [], 0
    for i, r in enumerate(results, 1):
        loc = r["path"]
        if r["page"]:
            loc += ", стр. %d" % r["page"]
        if r["t_start"] is not None:
            loc += ", время %s–%s" % (_fmt_time(r["t_start"]), _fmt_time(r["t_end"] or r["t_start"]))
        frag = r["text"][:3000]
        block = "[%d] Файл: %s\n%s" % (i, loc, frag)
        if used + len(block) > max_chars:
            break
        used += len(block)
        blocks.append(block)
    return "\n\n".join(blocks), len(blocks)


def ask(conn, emb, cfg, question, limit=8):
    from .search import search as _search

    results = _search(conn, emb, cfg, question, limit=limit)
    if not results:
        return {"answer": "В индексе ничего не найдено. Проиндексируйте диски: "
                          "python -m hds.cli index", "sources": []}
    max_chars = int(dig(cfg, "chat.max_context_chars", 14000))
    context, n_used = build_context(results, max_chars)
    url = dig(cfg, "chat.base_url", "http://localhost:1234/v1").rstrip("/") + "/chat/completions"
    payload = {
        "model": dig(cfg, "chat.model", "local-model"),
        "temperature": float(dig(cfg, "chat.temperature", 0.2)),
        # лимит генерации: без него локальная модель может писать минуты и
        # выходить за таймаут MCP-клиента (Cline режет вызов по "timeout")
        "max_tokens": int(dig(cfg, "chat.max_tokens", 600)),
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": "Вопрос пользователя: %s\n\n"
             "Фрагменты из локальных файлов:\n\n%s" % (question, context)},
        ],
    }
    try:
        r = requests.post(url, json=payload, timeout=600)
        r.raise_for_status()
        answer = r.json()["choices"][0]["message"]["content"]
    except Exception as e:  # noqa: BLE001
        return {
            "answer": "Не удалось получить ответ модели (%s). Найденные файлы:\n%s"
                      % (e, "\n".join(format_location(x) for x in results[:n_used])),
            "sources": results[:n_used],
        }
    return {"answer": answer, "sources": results[:n_used]}