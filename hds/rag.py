"""Ответ на свободный вопрос по локальным файлам (RAG через чат-роль llama-server).

llama-server в цепочке — только генератор текста: поле tools никогда не
передаётся (инструменты вызывает агент, а не модель — иначе дэдлоки),
нативный tool-calling сервера выключен (chat-инстанс запускается без --jinja).
"""
import re

import requests

from .config import dig
from .search import format_location, search

SYSTEM_PROMPT = (
    "Ты помогаешь искать информацию в локальных файлах пользователя. "
    "Отвечай на русском языке. Опирайся ТОЛЬКО на приведённые фрагменты документов; "
    "если их недостаточно — так и скажи. При упоминании фактов указывай источник в "
    "формате [N], где N — номер фрагмента. Отвечай компактно: суть в нескольких "
    "предложениях и короткий список источников (путь, страница/таймкод) в конце — "
    "ответ генерирует локальная модель, длинные ответы не помещаются в таймаут клиента. "
    "Не вызывай никаких инструментов, просто ответь текстом."
)

# thinking-модель может завершить генерацию ВНУТРИ блока размышлений:
# весь ответ остаётся в reasoning_content или в inline-тегах размышлений,
# content пуст. Один прозрачный дозапрос с подсказкой «дай финальный ответ»
# вместо пустого ответа клиенту (nudge не содержит данных — добавляется
# к уже отправленным сообщениям).
_CONTINUE_NUDGE = (
    "Твой предыдущий ответ не содержал финального текста — только размышления. "
    "Дай итоговый ответ на вопрос пользователя по приведённым фрагментам."
)

# Теги размышлений собираем конкатенацией, чтобы не держать в исходнике
# строки, похожие на разметку (часть редакторов/линтеров их «съедает»).
_T_OPEN = "<" + "think" + ">"
_T_CLOSE = "<" + "/" + "think" + ">"
_THINK_RX = re.compile(re.escape(_T_OPEN) + ".*?" + re.escape(_T_CLOSE)
                       + "|" + re.escape(_T_CLOSE) + ".*",
                       re.DOTALL)


def _strip_think(text):
    """Убрать inline-теги размышлений из content (бывают при thinking=auto,
    когда сервер не вынес размышления в reasoning_content)."""
    text = _THINK_RX.sub("", text or "")
    return text.strip()


def _chat_url(cfg):
    return dig(cfg, "chat.base_url", "http://127.0.0.1:8010/v1").rstrip("/") \
        + "/chat/completions"


def _chat_payload(cfg, messages):
    """Тело запроса /chat/completions.

    thinking=off (по умолчанию для RAG — быстрые ответы в таймаут Cline 300 с):
    chat_template_kwargs {"enable_thinking": false} — точное отключение
    размышлений на уровне шаблона Qwen3.x (то, чего не умел LM Studio).
    max_tokens остаётся: llama-server ограничивает им генерацию.
    """
    payload = {
        "model": dig(cfg, "chat.model", "qwen3.5-9b"),
        "temperature": float(dig(cfg, "chat.temperature", 0.2)),
        # лимит генерации: без него локальная модель может писать минуты и
        # выходить за таймаут MCP-клиента (Cline режет вызов по "timeout")
        "max_tokens": int(dig(cfg, "chat.max_tokens", 600)),
        "messages": messages,
    }
    if str(dig(cfg, "chat.thinking", "off") or "off").strip().lower() == "off":
        payload["chat_template_kwargs"] = {"enable_thinking": False}
    return payload


def _fmt_time(t):
    return "%02d:%02d:%02d" % (t // 3600, t % 3600 // 60, t % 60)


def build_context(results, max_chars):
    blocks, used = [], 0
    for i, r in enumerate(results, 1):
        loc = r["path"]
        if r["page"] is not None:  # страница 0 — валидная (не «нет страницы»)
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

    # реранкер (rerank.enabled, по умолчанию выключен): ищем кандидатов с запасом
    # (top-20), cross-encoder переставляет их и отбирает top-N для генерации
    pool = 20 if dig(cfg, "rerank.enabled", False) else limit
    results = _search(conn, emb, cfg, question, limit=pool)
    if not results:
        return {"answer": "В индексе ничего не найдено. Проиндексируйте диски: "
                          "python -m hds.cli index", "sources": []}
    if dig(cfg, "rerank.enabled", False):
        from .rerank import rerank_results

        reranked = rerank_results(cfg, question, results, top_n=limit)
        if reranked is not None:
            results = reranked
    max_chars = int(dig(cfg, "chat.max_context_chars", 14000))
    context, n_used = build_context(results, max_chars)
    messages = [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": "Вопрос пользователя: %s\n\n"
         "Фрагменты из локальных файлов:\n\n%s" % (question, context)},
    ]
    payload = _chat_payload(cfg, messages)
    url = _chat_url(cfg)
    # Таймаут согласован с MCP-клиентом (Cline режет вызов по 300 с): один
    # запрос должен укладываться с запасом; раньше 600 с + nudge-повтор
    # давали до 1200 с «молчаливого» зависания ask_my_files.
    timeout = float(dig(cfg, "chat.timeout", 240))

    def _post(p):
        r = requests.post(url, json=p, timeout=timeout)
        r.raise_for_status()
        return r.json()["choices"][0]["message"] or {}

    try:
        msg = _post(payload)
        # thinking=auto: размышления в reasoning_content не нужны клиенту;
        # content может быть пуст при завершении внутри блока размышлений —
        # один nudge-дозапрос (см. anonymizer_proxy llm_router)
        content = _strip_think(msg.get("content") or "")
        if not content and msg.get("reasoning_content"):
            nudge = dict(payload)
            nudge["messages"] = messages + [{"role": "user",
                                             "content": _CONTINUE_NUDGE}]
            content = _strip_think(_post(nudge).get("content") or "")
        answer = content
    except Exception as e:  # noqa: BLE001
        return {
            "answer": "Не удалось получить ответ модели (%s). Найденные файлы:\n%s"
                      % (e, "\n".join(format_location(x) for x in results[:n_used])),
            "sources": results[:n_used],
        }
    return {"answer": answer, "sources": results[:n_used]}